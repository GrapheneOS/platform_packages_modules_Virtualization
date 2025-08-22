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

//! Page table management.

use crate::mmu::MmuError;
use core::ops::Range;
use core::result;

type Result<T> = result::Result<T, MmuError>;

/// High-level API for managing MMU mappings.
#[derive(Default)]
pub struct PageTable {}

impl Drop for PageTable {
    fn drop(&mut self) {
        // TODO(b/354116267): implement for x86_64
    }
}

impl PageTable {
    /// Activates the page table.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the PageTable instance has valid and identical mappings for the
    /// code being currently executed. Otherwise, the Rust execution model (on which the borrow
    /// checker relies) would be violated.
    pub unsafe fn activate(&mut self) {}

    /// Maps the given range of virtual addresses to the physical addresses as lazily mapped
    /// nGnRE device memory.
    pub fn mark_as_lazy_device(&mut self, _range: &Range<usize>) -> Result<()> {
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as valid device
    /// nGnRE device memory.
    pub fn map_device(&mut self, _range: &Range<usize>) -> Result<()> {
        Ok(())
    }

    /// Modify the PTEs corresponding to a given range from (invalid) "lazy MMIO" to valid MMIO.
    ///
    /// Returns an error if any PTE in the range is not an invalid lazy MMIO mapping.
    pub fn map_device_expect_lazy(&mut self, _range: &Range<usize>) -> Result<()> {
        // TODO(b/362733888): Provide the implementation for x86_64
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as non-executable
    /// and writable normal memory.
    pub fn map_data(&mut self, _range: &Range<usize>) -> Result<()> {
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as non-executable,
    /// read-only and writable-clean normal memory.
    pub fn map_data_track_dirty_state(&mut self, _range: &Range<usize>) -> Result<()> {
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as read-only
    /// normal memory.
    pub fn map_code(&mut self, _range: &Range<usize>) -> Result<()> {
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as non-executable
    /// and read-only normal memory.
    pub fn map_rodata(&mut self, _range: &Range<usize>) -> Result<()> {
        Ok(())
    }

    /// Marks a previously-registered R/W region as "dirty" i.e. it has been written to.
    pub(crate) fn mark_data_dirty(&mut self, _range: &Range<usize>) -> Result<()> {
        Ok(())
    }

    pub(crate) fn sync_dirty_state(&mut self) -> Result<()> {
        Ok(())
    }

    pub(crate) fn flush_dirty_pages(&mut self, _range: &Range<usize>) -> Result<()> {
        // TODO(b/362733888): Provide the implementation for x86_64
        Ok(())
    }
}
