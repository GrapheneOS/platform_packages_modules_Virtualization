// Copyright 2023, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Shared memory management.

use super::error::MemoryTrackerError;
use super::util::virt_to_phys;
use crate::layout;
use crate::util::unchecked_align_down;
use aarch64_paging::paging::{MemoryRegion as VaRange, VirtualAddress, PAGE_SIZE};
use alloc::alloc::{alloc_zeroed, dealloc, handle_alloc_error};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use buddy_system_allocator::{FrameAllocator, LockedFrameAllocator};
use core::alloc::Layout;
use core::cmp::max;
use core::ops::Range;
use core::ptr::NonNull;
use core::result;
use hypervisor_backends::{self, get_mem_sharer, get_mmio_guard};
use log::trace;
use once_cell::race::OnceBox;
use spin::mutex::SpinMutex;

pub(crate) static SHARED_POOL: OnceBox<LockedFrameAllocator<32>> = OnceBox::new();
pub(crate) static SHARED_MEMORY: SpinMutex<Option<MemorySharer>> = SpinMutex::new(None);

/// Memory range.
pub type MemoryRange = Range<usize>;

type Result<T> = result::Result<T, MemoryTrackerError>;

pub(crate) struct MmioSharer {
    granule: usize,
    frames: BTreeSet<usize>,
}

impl MmioSharer {
    pub fn new() -> Result<Self> {
        let granule = Self::get_granule()?;
        let frames = BTreeSet::new();

        // Allows safely calling util::unchecked_align_down().
        assert!(granule.is_power_of_two());

        Ok(Self { granule, frames })
    }

    pub fn get_granule() -> Result<usize> {
        let Some(mmio_guard) = get_mmio_guard() else {
            return Ok(PAGE_SIZE);
        };
        match mmio_guard.granule()? {
            granule if granule % PAGE_SIZE == 0 => Ok(granule), // For good measure.
            granule => Err(MemoryTrackerError::UnsupportedMmioGuardGranule(granule)),
        }
    }

    /// Share the MMIO region aligned to the granule size containing addr (not validated as MMIO).
    pub fn share(&mut self, addr: VirtualAddress) -> Result<VaRange> {
        // This can't use virt_to_phys() since 0x0 is a valid MMIO address and we are ID-mapped.
        let phys = addr.0;
        let base = unchecked_align_down(phys, self.granule);

        // TODO(ptosi): Share the UART using this method and remove the hardcoded check.
        if self.frames.contains(&base) || base == layout::UART_PAGE_ADDR {
            return Err(MemoryTrackerError::DuplicateMmioShare(base));
        }

        if let Some(mmio_guard) = get_mmio_guard() {
            mmio_guard.map(base)?;
        }

        let inserted = self.frames.insert(base);
        assert!(inserted);

        let base_va = VirtualAddress(base);
        Ok((base_va..base_va + self.granule).into())
    }

    pub fn unshare_all(&mut self) {
        let Some(mmio_guard) = get_mmio_guard() else {
            return self.frames.clear();
        };

        while let Some(base) = self.frames.pop_first() {
            mmio_guard.unmap(base).unwrap();
        }
    }
}

impl Drop for MmioSharer {
    fn drop(&mut self) {
        self.unshare_all();
    }
}

/// Allocates a memory range of at least the given size and alignment that is shared with the host.
/// Returns a pointer to the buffer.
pub(crate) fn alloc_shared(layout: Layout) -> hypervisor_backends::Result<NonNull<u8>> {
    assert_ne!(layout.size(), 0);
    let Some(buffer) = try_shared_alloc(layout) else {
        handle_alloc_error(layout);
    };

    trace!("Allocated shared buffer at {buffer:?} with {layout:?}");
    Ok(buffer)
}

fn try_shared_alloc(layout: Layout) -> Option<NonNull<u8>> {
    let mut shared_pool = SHARED_POOL.get().unwrap().lock();

    if let Some(buffer) = shared_pool.alloc_aligned(layout) {
        Some(NonNull::new(buffer as _).unwrap())
    } else if let Some(shared_memory) = SHARED_MEMORY.lock().as_mut() {
        // Adjusts the layout size to the max of the next power of two and the alignment,
        // as this is the actual size of the memory allocated in `alloc_aligned()`.
        let size = max(layout.size().next_power_of_two(), layout.align());
        let refill_layout = Layout::from_size_align(size, layout.align()).unwrap();
        shared_memory.refill(&mut shared_pool, refill_layout);
        shared_pool.alloc_aligned(layout).map(|buffer| NonNull::new(buffer as _).unwrap())
    } else {
        None
    }
}

/// Unshares and deallocates a memory range which was previously allocated by `alloc_shared`.
///
/// The layout passed in must be the same layout passed to the original `alloc_shared` call.
///
/// # Safety
///
/// The memory must have been allocated by `alloc_shared` with the same layout, and not yet
/// deallocated.
pub(crate) unsafe fn dealloc_shared(
    vaddr: NonNull<u8>,
    layout: Layout,
) -> hypervisor_backends::Result<()> {
    SHARED_POOL.get().unwrap().lock().dealloc_aligned(vaddr.as_ptr() as usize, layout);

    trace!("Deallocated shared buffer at {vaddr:?} with {layout:?}");
    Ok(())
}

/// Allocates memory on the heap and shares it with the host.
///
/// Unshares all pages when dropped.
pub(crate) struct MemorySharer {
    granule: usize,
    frames: Vec<(usize, Layout)>,
}

impl MemorySharer {
    /// Constructs a new `MemorySharer` instance with the specified granule size and capacity.
    /// `granule` must be a power of 2.
    pub fn new(granule: usize, capacity: usize) -> Self {
        assert!(granule.is_power_of_two());
        Self { granule, frames: Vec::with_capacity(capacity) }
    }

    /// Gets from the global allocator a granule-aligned region that suits `hint` and share it.
    pub fn refill(&mut self, pool: &mut FrameAllocator<32>, hint: Layout) {
        let layout = hint.align_to(self.granule).unwrap().pad_to_align();
        assert_ne!(layout.size(), 0);
        // SAFETY: layout has non-zero size.
        let Some(shared) = NonNull::new(unsafe { alloc_zeroed(layout) }) else {
            handle_alloc_error(layout);
        };

        let base = shared.as_ptr() as usize;
        let end = base.checked_add(layout.size()).unwrap();

        if let Some(mem_sharer) = get_mem_sharer() {
            trace!("Sharing memory region {:#x?}", base..end);
            for vaddr in (base..end).step_by(self.granule) {
                let vaddr = NonNull::new(vaddr as *mut _).unwrap();
                mem_sharer.share(virt_to_phys(vaddr).try_into().unwrap()).unwrap();
            }
        }

        self.frames.push((base, layout));
        pool.add_frame(base, end);
    }
}

impl Drop for MemorySharer {
    fn drop(&mut self) {
        while let Some((base, layout)) = self.frames.pop() {
            if let Some(mem_sharer) = get_mem_sharer() {
                let end = base.checked_add(layout.size()).unwrap();
                trace!("Unsharing memory region {:#x?}", base..end);
                for vaddr in (base..end).step_by(self.granule) {
                    let vaddr = NonNull::new(vaddr as *mut _).unwrap();
                    mem_sharer.unshare(virt_to_phys(vaddr).try_into().unwrap()).unwrap();
                }
            }

            // SAFETY: The region was obtained from alloc_zeroed() with the recorded layout.
            unsafe { dealloc(base as *mut _, layout) };
        }
    }
}
