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

//! HAL for the virtio_drivers crate.

use super::pci::PCI_INFO;
use crate::memory::{alloc_shared, dealloc_shared, phys_to_virt, virt_to_phys};
use crate::util::RangeExt as _;
use core::alloc::Layout;
use core::mem::size_of;
use core::ptr::{copy_nonoverlapping, NonNull};
use log::trace;
use virtio_drivers::{BufferDirection, Hal, PhysAddr, PAGE_SIZE};

/// The alignment to use for the temporary buffers allocated by `HalImpl::share`. There doesn't seem
/// to be any particular alignment required by VirtIO for these, so 16 bytes should be enough to
/// allow appropriate alignment for whatever fields are accessed. `alloc_shared` will increase the
/// alignment to the memory sharing granule size anyway.
const SHARED_BUFFER_ALIGNMENT: usize = size_of::<u128>();

/// HAL implementation for the virtio_drivers crate.
pub struct HalImpl;

/// SAFETY: See the 'Implementation Safety' comments on methods below for how they fulfill the
/// safety requirements of the unsafe `Hal` trait.
unsafe impl Hal for HalImpl {
    /// # Implementation Safety
    ///
    /// `dma_alloc` ensures the returned DMA buffer is not aliased with any other allocation or
    /// reference in the program until it is deallocated by `dma_dealloc` by allocating a unique
    /// block of memory using `alloc_shared`, which is guaranteed to allocate valid and unique
    /// memory. We request an alignment of at least `PAGE_SIZE` from `alloc_shared`. We zero the
    /// buffer before returning it.
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let layout = dma_layout(pages);
        let vaddr =
            alloc_shared(layout).expect("Failed to allocate and share VirtIO DMA range with host");
        // SAFETY: vaddr points to a region allocated for the caller so is safe to access.
        unsafe { core::ptr::write_bytes(vaddr.as_ptr(), 0, layout.size()) };
        let paddr = virt_to_phys(vaddr);
        (paddr, vaddr)
    }

    unsafe fn dma_dealloc(_paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        // SAFETY: Memory was allocated by `dma_alloc` using `alloc_shared` with the same layout.
        unsafe { dealloc_shared(vaddr, dma_layout(pages)) }
            .expect("Failed to unshare VirtIO DMA range with host");
        0
    }

    /// # Implementation Safety
    ///
    /// The returned pointer must be valid because the `paddr` describes a valid MMIO region, we
    /// check that it is within the PCI MMIO range, and we previously mapped the entire PCI MMIO
    /// range. It can't alias any other allocations because we previously validated in
    /// `map_mmio_range` that the PCI MMIO range didn't overlap with any other memory ranges.
    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        let pci_info = PCI_INFO.get().expect("VirtIO HAL used before PCI_INFO was initialized");
        let bar_range = {
            let start = pci_info.bar_range.start.try_into().unwrap();
            let end = pci_info.bar_range.end.try_into().unwrap();

            start..end
        };
        let mmio_range = paddr..paddr.checked_add(size).expect("PCI MMIO region end overflowed");

        // Check that the region is within the PCI MMIO range that we read from the device tree. If
        // not, the host is probably trying to do something malicious.
        assert!(
            mmio_range.is_within(&bar_range),
            "PCI MMIO region was outside of expected BAR range.",
        );

        phys_to_virt(paddr)
    }

    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
        let size = buffer.len();

        let bounce = alloc_shared(bb_layout(size))
            .expect("Failed to allocate and share VirtIO bounce buffer with host");
        let paddr = virt_to_phys(bounce);
        if direction != BufferDirection::DeviceToDriver {
            let src = buffer.cast::<u8>().as_ptr().cast_const();
            trace!("VirtIO bounce buffer at {bounce:?} (PA:{paddr:#x}) initialized from {src:?}");
            // SAFETY: Both regions are valid, properly aligned, and don't overlap.
            unsafe { copy_nonoverlapping(src, bounce.as_ptr(), size) };
        }

        paddr
    }

    unsafe fn unshare(paddr: PhysAddr, buffer: NonNull<[u8]>, direction: BufferDirection) {
        let bounce = phys_to_virt(paddr);
        let size = buffer.len();
        if direction != BufferDirection::DriverToDevice {
            let dest = buffer.cast::<u8>().as_ptr();
            trace!("VirtIO bounce buffer at {bounce:?} (PA:{paddr:#x}) copied back to {dest:?}");
            // SAFETY: Both regions are valid, properly aligned, and don't overlap.
            unsafe { copy_nonoverlapping(bounce.as_ptr(), dest, size) };
        }

        // SAFETY: Memory was allocated by `share` using `alloc_shared` with the same layout.
        unsafe { dealloc_shared(bounce, bb_layout(size)) }
            .expect("Failed to unshare and deallocate VirtIO bounce buffer");
    }
}

fn dma_layout(pages: usize) -> Layout {
    let size = pages.checked_mul(PAGE_SIZE).unwrap();
    Layout::from_size_align(size, PAGE_SIZE).unwrap()
}

fn bb_layout(size: usize) -> Layout {
    Layout::from_size_align(size, SHARED_BUFFER_ALIGNMENT).unwrap()
}
