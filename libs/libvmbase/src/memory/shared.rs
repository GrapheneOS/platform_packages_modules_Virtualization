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

use super::dbm::{flush_dirty_range, mark_dirty_block, set_dbm_enabled};
use super::error::MemoryTrackerError;
use super::page_table::{PageTable, MMIO_LAZY_MAP_FLAG};
use super::util::virt_to_phys;
use crate::dsb;
use crate::exceptions::HandleExceptionError;
use crate::hyp::{self, get_mem_sharer, get_mmio_guard};
use crate::layout;
use crate::util::unchecked_align_down;
use crate::util::RangeExt as _;
use aarch64_paging::paging::{
    Attributes, Descriptor, MemoryRegion as VaRange, VirtualAddress, PAGE_SIZE,
};
use alloc::alloc::{alloc_zeroed, dealloc, handle_alloc_error};
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use buddy_system_allocator::{FrameAllocator, LockedFrameAllocator};
use core::alloc::Layout;
use core::cmp::max;
use core::mem::size_of;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::ptr::NonNull;
use core::result;
use log::{debug, error, trace};
use once_cell::race::OnceBox;
use spin::mutex::SpinMutex;
use tinyvec::ArrayVec;

/// A global static variable representing the system memory tracker, protected by a spin mutex.
pub static MEMORY: SpinMutex<Option<MemoryTracker>> = SpinMutex::new(None);

static SHARED_POOL: OnceBox<LockedFrameAllocator<32>> = OnceBox::new();
static SHARED_MEMORY: SpinMutex<Option<MemorySharer>> = SpinMutex::new(None);

/// Memory range.
pub type MemoryRange = Range<usize>;

fn get_va_range(range: &MemoryRange) -> VaRange {
    VaRange::new(range.start, range.end)
}

type Result<T> = result::Result<T, MemoryTrackerError>;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum MemoryType {
    #[default]
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Debug, Default)]
struct MemoryRegion {
    range: MemoryRange,
    mem_type: MemoryType,
}

/// Tracks non-overlapping slices of main memory.
pub struct MemoryTracker {
    total: MemoryRange,
    page_table: PageTable,
    regions: ArrayVec<[MemoryRegion; MemoryTracker::CAPACITY]>,
    mmio_regions: ArrayVec<[MemoryRange; MemoryTracker::MMIO_CAPACITY]>,
    mmio_range: MemoryRange,
    payload_range: Option<MemoryRange>,
    mmio_sharer: MmioSharer,
}

impl MemoryTracker {
    const CAPACITY: usize = 5;
    const MMIO_CAPACITY: usize = 5;

    /// Creates a new instance from an active page table, covering the maximum RAM size.
    pub fn new(
        mut page_table: PageTable,
        total: MemoryRange,
        mmio_range: MemoryRange,
        payload_range: Option<Range<VirtualAddress>>,
    ) -> Self {
        assert!(
            !total.overlaps(&mmio_range),
            "MMIO space should not overlap with the main memory region."
        );

        // Activate dirty state management first, otherwise we may get permission faults immediately
        // after activating the new page table. This has no effect before the new page table is
        // activated because none of the entries in the initial idmap have the DBM flag.
        set_dbm_enabled(true);

        debug!("Activating dynamic page table...");
        // SAFETY: page_table duplicates the static mappings for everything that the Rust code is
        // aware of so activating it shouldn't have any visible effect.
        unsafe { page_table.activate() }
        debug!("... Success!");

        Self {
            total,
            page_table,
            regions: ArrayVec::new(),
            mmio_regions: ArrayVec::new(),
            mmio_range,
            payload_range: payload_range.map(|r| r.start.0..r.end.0),
            mmio_sharer: MmioSharer::new().unwrap(),
        }
    }

    /// Resize the total RAM size.
    ///
    /// This function fails if it contains regions that are not included within the new size.
    pub fn shrink(&mut self, range: &MemoryRange) -> Result<()> {
        if range.start != self.total.start {
            return Err(MemoryTrackerError::DifferentBaseAddress);
        }
        if self.total.end < range.end {
            return Err(MemoryTrackerError::SizeTooLarge);
        }
        if !self.regions.iter().all(|r| r.range.is_within(range)) {
            return Err(MemoryTrackerError::SizeTooSmall);
        }

        self.total = range.clone();
        Ok(())
    }

    /// Allocate the address range for a const slice; returns None if failed.
    pub fn alloc_range(&mut self, range: &MemoryRange) -> Result<MemoryRange> {
        let region = MemoryRegion { range: range.clone(), mem_type: MemoryType::ReadOnly };
        self.check_allocatable(&region)?;
        self.page_table.map_rodata(&get_va_range(range)).map_err(|e| {
            error!("Error during range allocation: {e}");
            MemoryTrackerError::FailedToMap
        })?;
        self.add(region)
    }

    /// Allocates the address range for a const slice.
    ///
    /// # Safety
    ///
    /// Callers of this method need to ensure that the `range` is valid for mapping as read-only
    /// data.
    pub unsafe fn alloc_range_outside_main_memory(
        &mut self,
        range: &MemoryRange,
    ) -> Result<MemoryRange> {
        let region = MemoryRegion { range: range.clone(), mem_type: MemoryType::ReadOnly };
        self.check_no_overlap(&region)?;
        self.page_table.map_rodata(&get_va_range(range)).map_err(|e| {
            error!("Error during range allocation: {e}");
            MemoryTrackerError::FailedToMap
        })?;
        self.add(region)
    }

    /// Allocate the address range for a mutable slice; returns None if failed.
    pub fn alloc_range_mut(&mut self, range: &MemoryRange) -> Result<MemoryRange> {
        let region = MemoryRegion { range: range.clone(), mem_type: MemoryType::ReadWrite };
        self.check_allocatable(&region)?;
        self.page_table.map_data_dbm(&get_va_range(range)).map_err(|e| {
            error!("Error during mutable range allocation: {e}");
            MemoryTrackerError::FailedToMap
        })?;
        self.add(region)
    }

    /// Allocate the address range for a const slice; returns None if failed.
    pub fn alloc(&mut self, base: usize, size: NonZeroUsize) -> Result<MemoryRange> {
        self.alloc_range(&(base..(base + size.get())))
    }

    /// Allocate the address range for a mutable slice; returns None if failed.
    pub fn alloc_mut(&mut self, base: usize, size: NonZeroUsize) -> Result<MemoryRange> {
        self.alloc_range_mut(&(base..(base + size.get())))
    }

    /// Checks that the given range of addresses is within the MMIO region, and then maps it
    /// appropriately.
    pub fn map_mmio_range(&mut self, range: MemoryRange) -> Result<()> {
        if !range.is_within(&self.mmio_range) {
            return Err(MemoryTrackerError::OutOfRange);
        }
        if self.mmio_regions.iter().any(|r| range.overlaps(r)) {
            return Err(MemoryTrackerError::Overlaps);
        }
        if self.mmio_regions.len() == self.mmio_regions.capacity() {
            return Err(MemoryTrackerError::Full);
        }

        if get_mmio_guard().is_some() {
            self.page_table.map_device_lazy(&get_va_range(&range)).map_err(|e| {
                error!("Error during lazy MMIO device mapping: {e}");
                MemoryTrackerError::FailedToMap
            })?;
        } else {
            self.page_table.map_device(&get_va_range(&range)).map_err(|e| {
                error!("Error during MMIO device mapping: {e}");
                MemoryTrackerError::FailedToMap
            })?;
        }

        if self.mmio_regions.try_push(range).is_some() {
            return Err(MemoryTrackerError::Full);
        }

        Ok(())
    }

    /// Checks that the memory region meets the following criteria:
    /// - It is within the range of the `MemoryTracker`.
    /// - It does not overlap with any previously allocated regions.
    /// - The `regions` ArrayVec has sufficient capacity to add it.
    fn check_allocatable(&self, region: &MemoryRegion) -> Result<()> {
        if !region.range.is_within(&self.total) {
            return Err(MemoryTrackerError::OutOfRange);
        }
        self.check_no_overlap(region)
    }

    /// Checks that the given region doesn't overlap with any other previously allocated regions,
    /// and that the regions ArrayVec has capacity to add it.
    fn check_no_overlap(&self, region: &MemoryRegion) -> Result<()> {
        if self.regions.iter().any(|r| region.range.overlaps(&r.range)) {
            return Err(MemoryTrackerError::Overlaps);
        }
        if self.regions.len() == self.regions.capacity() {
            return Err(MemoryTrackerError::Full);
        }
        Ok(())
    }

    fn add(&mut self, region: MemoryRegion) -> Result<MemoryRange> {
        if self.regions.try_push(region).is_some() {
            return Err(MemoryTrackerError::Full);
        }

        Ok(self.regions.last().unwrap().range.clone())
    }

    /// Unshares any MMIO region previously shared with the MMIO guard.
    pub fn unshare_all_mmio(&mut self) -> Result<()> {
        self.mmio_sharer.unshare_all();

        Ok(())
    }

    /// Initialize the shared heap to dynamically share memory from the global allocator.
    pub fn init_dynamic_shared_pool(&mut self, granule: usize) -> Result<()> {
        const INIT_CAP: usize = 10;

        let previous = SHARED_MEMORY.lock().replace(MemorySharer::new(granule, INIT_CAP));
        if previous.is_some() {
            return Err(MemoryTrackerError::SharedMemorySetFailure);
        }

        SHARED_POOL
            .set(Box::new(LockedFrameAllocator::new()))
            .map_err(|_| MemoryTrackerError::SharedPoolSetFailure)?;

        Ok(())
    }

    /// Initialize the shared heap from a static region of memory.
    ///
    /// Some hypervisors such as Gunyah do not support a MemShare API for guest
    /// to share its memory with host. Instead they allow host to designate part
    /// of guest memory as "shared" ahead of guest starting its execution. The
    /// shared memory region is indicated in swiotlb node. On such platforms use
    /// a separate heap to allocate buffers that can be shared with host.
    pub fn init_static_shared_pool(&mut self, range: Range<usize>) -> Result<()> {
        let size = NonZeroUsize::new(range.len()).unwrap();
        let range = self.alloc_mut(range.start, size)?;
        let shared_pool = LockedFrameAllocator::<32>::new();

        shared_pool.lock().insert(range);

        SHARED_POOL
            .set(Box::new(shared_pool))
            .map_err(|_| MemoryTrackerError::SharedPoolSetFailure)?;

        Ok(())
    }

    /// Initialize the shared heap to use heap memory directly.
    ///
    /// When running on "non-protected" hypervisors which permit host direct accesses to guest
    /// memory, there is no need to perform any memory sharing and/or allocate buffers from a
    /// dedicated region so this function instructs the shared pool to use the global allocator.
    pub fn init_heap_shared_pool(&mut self) -> Result<()> {
        // As MemorySharer only calls MEM_SHARE methods if the hypervisor supports them, internally
        // using init_dynamic_shared_pool() on a non-protected platform will make use of the heap
        // without any actual "dynamic memory sharing" taking place and, as such, the granule may
        // be set to the one of the global_allocator i.e. a byte.
        self.init_dynamic_shared_pool(size_of::<u8>())
    }

    /// Unshares any memory that may have been shared.
    pub fn unshare_all_memory(&mut self) {
        drop(SHARED_MEMORY.lock().take());
    }

    /// Handles translation fault for blocks flagged for lazy MMIO mapping by enabling the page
    /// table entry and MMIO guard mapping the block. Breaks apart a block entry if required.
    fn handle_mmio_fault(&mut self, addr: VirtualAddress) -> Result<()> {
        let shared_range = self.mmio_sharer.share(addr)?;
        self.map_lazy_mmio_as_valid(&shared_range)?;

        Ok(())
    }

    /// Modify the PTEs corresponding to a given range from (invalid) "lazy MMIO" to valid MMIO.
    ///
    /// Returns an error if any PTE in the range is not an invalid lazy MMIO mapping.
    fn map_lazy_mmio_as_valid(&mut self, page_range: &VaRange) -> Result<()> {
        // This must be safe and free from break-before-make (BBM) violations, given that the
        // initial lazy mapping has the valid bit cleared, and each newly created valid descriptor
        // created inside the mapping has the same size and alignment.
        self.page_table
            .modify_range(page_range, &|_: &VaRange, desc: &mut Descriptor, _: usize| {
                let flags = desc.flags().expect("Unsupported PTE flags set");
                if flags.contains(MMIO_LAZY_MAP_FLAG) && !flags.contains(Attributes::VALID) {
                    desc.modify_flags(Attributes::VALID, Attributes::empty());
                    Ok(())
                } else {
                    Err(())
                }
            })
            .map_err(|_| MemoryTrackerError::InvalidPte)
    }

    /// Flush all memory regions marked as writable-dirty.
    fn flush_dirty_pages(&mut self) -> Result<()> {
        // Collect memory ranges for which dirty state is tracked.
        let writable_regions =
            self.regions.iter().filter(|r| r.mem_type == MemoryType::ReadWrite).map(|r| &r.range);
        // Execute a barrier instruction to ensure all hardware updates to the page table have been
        // observed before reading PTE flags to determine dirty state.
        dsb!("ish");
        // Now flush writable-dirty pages in those regions.
        for range in writable_regions.chain(self.payload_range.as_ref().into_iter()) {
            self.page_table
                .walk_range(&get_va_range(range), &flush_dirty_range)
                .map_err(|_| MemoryTrackerError::FlushRegionFailed)?;
        }
        Ok(())
    }

    /// Handles permission fault for read-only blocks by setting writable-dirty state.
    /// In general, this should be called from the exception handler when hardware dirty
    /// state management is disabled or unavailable.
    fn handle_permission_fault(&mut self, addr: VirtualAddress) -> Result<()> {
        self.page_table
            .modify_range(&(addr..addr + 1).into(), &mark_dirty_block)
            .map_err(|_| MemoryTrackerError::SetPteDirtyFailed)
    }
}

impl Drop for MemoryTracker {
    fn drop(&mut self) {
        set_dbm_enabled(false);
        self.flush_dirty_pages().unwrap();
        self.unshare_all_memory();
    }
}

struct MmioSharer {
    granule: usize,
    frames: BTreeSet<usize>,
}

impl MmioSharer {
    fn new() -> Result<Self> {
        let granule = Self::get_granule()?;
        let frames = BTreeSet::new();

        // Allows safely calling util::unchecked_align_down().
        assert!(granule.is_power_of_two());

        Ok(Self { granule, frames })
    }

    fn get_granule() -> Result<usize> {
        let Some(mmio_guard) = get_mmio_guard() else {
            return Ok(PAGE_SIZE);
        };
        match mmio_guard.granule()? {
            granule if granule % PAGE_SIZE == 0 => Ok(granule), // For good measure.
            granule => Err(MemoryTrackerError::UnsupportedMmioGuardGranule(granule)),
        }
    }

    /// Share the MMIO region aligned to the granule size containing addr (not validated as MMIO).
    fn share(&mut self, addr: VirtualAddress) -> Result<VaRange> {
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

    fn unshare_all(&mut self) {
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
pub(crate) fn alloc_shared(layout: Layout) -> hyp::Result<NonNull<u8>> {
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
pub(crate) unsafe fn dealloc_shared(vaddr: NonNull<u8>, layout: Layout) -> hyp::Result<()> {
    SHARED_POOL.get().unwrap().lock().dealloc_aligned(vaddr.as_ptr() as usize, layout);

    trace!("Deallocated shared buffer at {vaddr:?} with {layout:?}");
    Ok(())
}

/// Allocates memory on the heap and shares it with the host.
///
/// Unshares all pages when dropped.
struct MemorySharer {
    granule: usize,
    frames: Vec<(usize, Layout)>,
}

impl MemorySharer {
    /// Constructs a new `MemorySharer` instance with the specified granule size and capacity.
    /// `granule` must be a power of 2.
    fn new(granule: usize, capacity: usize) -> Self {
        assert!(granule.is_power_of_two());
        Self { granule, frames: Vec::with_capacity(capacity) }
    }

    /// Gets from the global allocator a granule-aligned region that suits `hint` and share it.
    fn refill(&mut self, pool: &mut FrameAllocator<32>, hint: Layout) {
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

/// Handles a translation fault with the given fault address register (FAR).
#[inline]
pub fn handle_translation_fault(far: VirtualAddress) -> result::Result<(), HandleExceptionError> {
    let mut guard = MEMORY.try_lock().ok_or(HandleExceptionError::PageTableUnavailable)?;
    let memory = guard.as_mut().ok_or(HandleExceptionError::PageTableNotInitialized)?;
    Ok(memory.handle_mmio_fault(far)?)
}

/// Handles a permission fault with the given fault address register (FAR).
#[inline]
pub fn handle_permission_fault(far: VirtualAddress) -> result::Result<(), HandleExceptionError> {
    let mut guard = MEMORY.try_lock().ok_or(HandleExceptionError::PageTableUnavailable)?;
    let memory = guard.as_mut().ok_or(HandleExceptionError::PageTableNotInitialized)?;
    Ok(memory.handle_permission_fault(far)?)
}
