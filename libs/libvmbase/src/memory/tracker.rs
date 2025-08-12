// Copyright 2024, The Android Open Source Project
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

//! Memory management.

use super::error::MemoryTrackerError;
use super::shared::{SHARED_MEMORY, SHARED_POOL};
#[cfg(target_arch = "aarch64")]
use crate::arch::aarch64::page_table::PageTable;
#[cfg(target_arch = "x86_64")]
use crate::arch::x86_64::page_table::PageTable;
use crate::layout;
use crate::memory::shared::{MemorySharer, MmioSharer};
use crate::mmu::MmuOps;
use crate::util::RangeExt as _;
use alloc::boxed::Box;
use buddy_system_allocator::LockedFrameAllocator;
use core::mem::size_of;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::result;
use hypervisor_backends::{get_mem_sharer, get_mmio_guard};
use log::{debug, error, info};
use spin::mutex::{SpinMutex, SpinMutexGuard};
use tinyvec::ArrayVec;

type GlobalMemoryTracker = MemoryTracker<PageTable>;

/// A global static variable representing the system memory tracker, protected by a spin mutex.
static MEMORY: SpinMutex<Option<GlobalMemoryTracker>> = SpinMutex::new(None);

type Result<T> = result::Result<T, MemoryTrackerError>;

/// Attempts to lock `MEMORY`, returns an error if already deactivated.
fn try_lock_memory_tracker() -> Result<SpinMutexGuard<'static, Option<GlobalMemoryTracker>>> {
    // Being single-threaded, we only spin if `deactivate_dynamic_page_tables()` leaked the lock.
    MEMORY.try_lock().ok_or(MemoryTrackerError::Unavailable)
}

/// Switch the MMU to the dynamic page tables, enabling mapping more regions.
///
/// Panics if called more than once.
pub(crate) fn switch_to_dynamic_page_tables() {
    let mut locked_tracker = try_lock_memory_tracker().unwrap();
    if locked_tracker.is_some() {
        panic!("switch_to_dynamic_page_tables() called more than once.");
    }

    locked_tracker.replace(MemoryTracker::new(
        layout::crosvm::MEM_START..layout::MAX_VIRT_ADDR,
        layout::crosvm::MMIO_RANGE,
    ));
}

/// Switch the MMU back to the static page tables (see `idmap` C symbol).
///
/// Panics if called before `switch_to_dynamic_page_tables()` or more than once.
pub fn deactivate_dynamic_page_tables() {
    let locked_tracker = try_lock_memory_tracker().unwrap();
    // Force future calls to try_lock_memory_tracker() to fail by leaking this lock guard.
    let leaked_tracker = SpinMutexGuard::leak(locked_tracker);
    // Force deallocation/unsharing of all the resources used by the MemoryTracker.
    drop(leaked_tracker.take())
}

/// Redefines the actual mappable range of memory.
///
/// Fails if a region has already been mapped beyond the new upper limit.
pub fn resize_available_memory(memory_range: &Range<usize>) -> Result<()> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    tracker.shrink(memory_range)
}

/// Initialize the memory pool for page sharing with the host.
pub fn init_shared_pool(static_range: Option<&Range<usize>>) -> Result<()> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    if let Some(mem_sharer) = get_mem_sharer() {
        let granule = mem_sharer.granule()?;
        tracker.init_dynamic_shared_pool(granule)
    } else if let Some(r) = static_range {
        tracker.init_static_shared_pool(r)
    } else {
        info!("Initialized shared pool from heap memory without MEM_SHARE");
        tracker.init_heap_shared_pool()
    }
}

/// Unshare all MMIO that was previously shared with the host, with the exception of the UART page.
pub fn unshare_all_mmio_except_uart() -> Result<()> {
    let Ok(mut locked_tracker) = try_lock_memory_tracker() else { return Ok(()) };
    let Some(tracker) = locked_tracker.as_mut() else { return Ok(()) };
    if cfg!(feature = "compat_android_13") {
        info!("Expecting a bug making MMIO_GUARD_UNMAP return NOT_SUPPORTED on success");
    }
    tracker.unshare_all_mmio()
}

/// Unshare all memory that was previously shared with the host.
pub fn unshare_all_memory() {
    let Ok(mut locked_tracker) = try_lock_memory_tracker() else { return };
    let Some(tracker) = locked_tracker.as_mut() else { return };
    tracker.unshare_all_memory()
}

/// Unshare the UART page, previously shared with the host.
pub fn unshare_uart() -> Result<()> {
    let Some(mmio_guard) = get_mmio_guard() else { return Ok(()) };
    let Some(console_uart_page) = layout::console_uart_page() else { return Ok(()) };
    Ok(mmio_guard.unmap(console_uart_page.start)?)
}

/// Map the provided range as normal memory, with R/W permissions.
///
/// This fails if the range has already been (partially) mapped.
pub fn map_data(addr: usize, size: NonZeroUsize) -> Result<()> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    let _ = tracker.map_data(addr, size)?;
    Ok(())
}

/// Map the provided range as normal memory, with R/W permissions.
///
/// Unlike `map_data()`, `deactivate_dynamic_page_tables()` will not flush caches for the range.
///
/// This fails if the range has already been (partially) mapped.
pub fn map_data_noflush(addr: usize, size: NonZeroUsize) -> Result<()> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    let _ = tracker.map_data_noflush(addr, size)?;
    Ok(())
}

/// Map the region potentially holding data appended to the image, with read-write permissions.
///
/// This fails if the footer has already been mapped.
pub fn map_image_footer() -> Result<Range<usize>> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    let range = tracker.map_image_footer()?;
    Ok(range)
}

/// Map the provided range as normal memory, with read-only permissions.
///
/// This fails if the range has already been (partially) mapped.
pub fn map_rodata(addr: usize, size: NonZeroUsize) -> Result<()> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    let _ = tracker.map_rodata(addr, size)?;
    Ok(())
}

// TODO(ptosi): Merge this into map_rodata.
/// Map the provided range as normal memory, with read-only permissions.
///
/// # Safety
///
/// Callers of this method need to ensure that the `range` is valid for mapping as read-only data.
pub unsafe fn map_rodata_outside_main_memory(addr: usize, size: NonZeroUsize) -> Result<()> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    let end = addr + usize::from(size);
    // SAFETY: Caller has checked that it is valid to map the range.
    let _ = unsafe { tracker.map_rodata_range_outside_main_memory(&(addr..end)) }?;
    Ok(())
}

/// Map the provided range as device memory.
///
/// This fails if the range has already been (partially) mapped.
pub fn map_device(addr: usize, size: NonZeroUsize) -> Result<()> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    let range = addr..(addr + usize::from(size));
    tracker.map_mmio_range(&range)
}

/// Handles a permission fault for a write to a data region, mapped as RO to track the dirty state.
pub(crate) fn handle_read_only_fault(addr: usize) -> Result<()> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    tracker.handle_permission_fault(addr)?;
    Ok(())
}

/// Handler for faults triggered by accesses to MMIO, previously marked as lazy MMIO.
pub(crate) fn handle_lazy_mmio_fault(addr: usize) -> Result<()> {
    let mut locked_tracker = try_lock_memory_tracker()?;
    let tracker = locked_tracker.as_mut().ok_or(MemoryTrackerError::Unavailable)?;
    tracker.handle_mmio_fault(addr)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum MemoryType {
    #[default]
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Debug, Default)]
struct TrackedRegion {
    range: Range<usize>,
    mem_type: MemoryType,
}

impl TrackedRegion {
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }
}

/// Tracks non-overlapping slices of main memory.
struct MemoryTracker<T: MmuOps> {
    total: Range<usize>,
    page_table: T,
    regions: ArrayVec<[TrackedRegion; 5]>,
    mmio_regions: ArrayVec<[Range<usize>; 5]>,
    mmio_range: Range<usize>,
    image_footer: Option<Range<usize>>,
    mmio_sharer: MmioSharer,
}

impl<T: MmuOps> MemoryTracker<T> {
    /// Creates a new instance from an active page table, covering the maximum RAM size.
    fn new(total: Range<usize>, mmio_range: Range<usize>) -> Self {
        assert!(
            !total.overlaps(&mmio_range),
            "MMIO space should not overlap with the main memory region."
        );

        let mut page_table = T::clone_static_page_tables();

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
            image_footer: None,
            mmio_sharer: MmioSharer::new().unwrap(),
        }
    }

    /// Resize the total RAM size.
    ///
    /// This function fails if it contains regions that are not included within the new size.
    fn shrink(&mut self, range: &Range<usize>) -> Result<()> {
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

    fn map_rodata(&mut self, base: usize, size: NonZeroUsize) -> Result<Range<usize>> {
        let region =
            TrackedRegion { range: base..(base + size.get()), mem_type: MemoryType::ReadOnly };
        self.check_allocatable(&region)?;
        self.page_table.map_rodata(&region.range()).map_err(|e| {
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
    unsafe fn map_rodata_range_outside_main_memory(
        &mut self,
        range: &Range<usize>,
    ) -> Result<Range<usize>> {
        let region = TrackedRegion { range: range.clone(), mem_type: MemoryType::ReadOnly };
        self.check_no_overlap(&region)?;
        self.page_table.map_rodata(&region.range()).map_err(|e| {
            error!("Error during range allocation: {e}");
            MemoryTrackerError::FailedToMap
        })?;
        self.add(region)
    }

    fn map_data(&mut self, base: usize, size: NonZeroUsize) -> Result<Range<usize>> {
        let range = base..(base + size.get());
        let region = TrackedRegion { range: range.clone(), mem_type: MemoryType::ReadWrite };
        self.check_allocatable(&region)?;
        self.page_table.map_data_track_dirty_state(&region.range()).map_err(|e| {
            error!("Error during mutable range allocation: {e}");
            MemoryTrackerError::FailedToMap
        })?;
        self.add(region)
    }

    fn map_data_noflush(&mut self, base: usize, size: NonZeroUsize) -> Result<Range<usize>> {
        let range = base..(base + size.get());
        let region = TrackedRegion { range: range.clone(), mem_type: MemoryType::ReadWrite };
        self.check_allocatable(&region)?;
        self.page_table.map_data(&region.range()).map_err(|e| {
            error!("Error during non-flushed mutable range allocation: {e}");
            MemoryTrackerError::FailedToMap
        })?;
        self.add(region)
    }

    /// Maps the image footer, with read-write permissions.
    fn map_image_footer(&mut self) -> Result<Range<usize>> {
        if self.image_footer.is_some() {
            return Err(MemoryTrackerError::FooterAlreadyMapped);
        }
        let range = layout::image_footer_range();
        self.page_table.map_data_track_dirty_state(&range).map_err(|e| {
            error!("Error during image footer map: {e}");
            MemoryTrackerError::FailedToMap
        })?;
        self.image_footer = Some(range.clone());
        Ok(range)
    }

    /// Checks that the given range of addresses is within the MMIO region, and then maps it
    /// appropriately.
    fn map_mmio_range(&mut self, range: &Range<usize>) -> Result<()> {
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
            self.page_table.mark_as_lazy_device(range).map_err(|e| {
                error!("Error during lazy MMIO device mapping: {e}");
                MemoryTrackerError::FailedToMap
            })?;
        } else {
            self.page_table.map_device(range).map_err(|e| {
                error!("Error during MMIO device mapping: {e}");
                MemoryTrackerError::FailedToMap
            })?;
        }

        if self.mmio_regions.try_push(range.clone()).is_some() {
            return Err(MemoryTrackerError::Full);
        }

        Ok(())
    }

    /// Checks that the memory region meets the following criteria:
    /// - It is within the range of the `MemoryTracker`.
    /// - It does not overlap with any previously allocated regions.
    /// - The `regions` ArrayVec has sufficient capacity to add it.
    fn check_allocatable(&self, region: &TrackedRegion) -> Result<()> {
        if !region.range.is_within(&self.total) {
            return Err(MemoryTrackerError::OutOfRange);
        }
        self.check_no_overlap(region)
    }

    /// Checks that the given region doesn't overlap with any other previously allocated regions,
    /// and that the regions ArrayVec has capacity to add it.
    fn check_no_overlap(&self, region: &TrackedRegion) -> Result<()> {
        if self.regions.iter().any(|r| region.range.overlaps(&r.range)) {
            return Err(MemoryTrackerError::Overlaps);
        }
        if self.regions.len() == self.regions.capacity() {
            return Err(MemoryTrackerError::Full);
        }
        Ok(())
    }

    fn add(&mut self, region: TrackedRegion) -> Result<Range<usize>> {
        if self.regions.try_push(region).is_some() {
            return Err(MemoryTrackerError::Full);
        }

        Ok(self.regions.last().unwrap().range.clone())
    }

    /// Unshares any MMIO region previously shared with the MMIO guard.
    fn unshare_all_mmio(&mut self) -> Result<()> {
        self.mmio_sharer.unshare_all();

        Ok(())
    }

    /// Initialize the shared heap to dynamically share memory from the global allocator.
    fn init_dynamic_shared_pool(&mut self, granule: usize) -> Result<()> {
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
    fn init_static_shared_pool(&mut self, range: &Range<usize>) -> Result<()> {
        let size = NonZeroUsize::new(range.len()).unwrap();
        let range = self.map_data(range.start, size)?;
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
    fn init_heap_shared_pool(&mut self) -> Result<()> {
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
    pub(crate) fn handle_mmio_fault(&mut self, addr: usize) -> Result<()> {
        let shared_range = self.mmio_sharer.share(addr)?;
        self.page_table
            .map_device_expect_lazy(&shared_range)
            .map_err(|_| MemoryTrackerError::InvalidPte)?;

        Ok(())
    }

    /// Flush all memory regions that may have been written to.
    fn flush_dirty_pages(&mut self) -> Result<()> {
        self.page_table.sync_dirty_state().map_err(|_| MemoryTrackerError::FlushRegionFailed)?;
        for region in &self.regions {
            if matches!(region.mem_type, MemoryType::ReadWrite) {
                self.page_table
                    .flush_dirty_pages(&region.range())
                    .map_err(|_| MemoryTrackerError::FlushRegionFailed)?;
            }
        }
        if let Some(range) = &self.image_footer {
            self.page_table
                .flush_dirty_pages(range)
                .map_err(|_| MemoryTrackerError::FlushRegionFailed)?;
        }
        Ok(())
    }

    /// Handles permission fault for read-only blocks by setting writable-dirty state.
    /// In general, this should be called from the exception handler when hardware dirty
    /// state management is disabled or unavailable.
    pub(crate) fn handle_permission_fault(&mut self, addr: usize) -> Result<()> {
        self.page_table
            .mark_data_dirty(&(addr..addr + 1))
            .map_err(|_| MemoryTrackerError::SetPteDirtyFailed)
    }
}

impl<T: MmuOps> Drop for MemoryTracker<T> {
    fn drop(&mut self) {
        self.flush_dirty_pages().unwrap();
        self.unshare_all_memory();
    }
}
