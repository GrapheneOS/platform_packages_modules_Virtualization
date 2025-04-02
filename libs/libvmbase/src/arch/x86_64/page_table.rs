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

use crate::arch::x86_64::paging::{Descriptor, MapError, MemoryRegion};
use core::result;

type Result<T> = result::Result<T, MapError>;

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
    pub fn map_device_lazy(&mut self, _range: &MemoryRegion) -> Result<()> {
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as valid device
    /// nGnRE device memory.
    pub fn map_device(&mut self, _range: &MemoryRegion) -> Result<()> {
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as non-executable
    /// and writable normal memory.
    pub fn map_data(&mut self, _range: &MemoryRegion) -> Result<()> {
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as non-executable,
    /// read-only and writable-clean normal memory.
    pub fn map_data_dbm(&mut self, _range: &MemoryRegion) -> Result<()> {
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as read-only
    /// normal memory.
    pub fn map_code(&mut self, _range: &MemoryRegion) -> Result<()> {
        Ok(())
    }

    /// Maps the given range of virtual addresses to the physical addresses as non-executable
    /// and read-only normal memory.
    pub fn map_rodata(&mut self, _range: &MemoryRegion) -> Result<()> {
        Ok(())
    }

    /// Applies the provided updater function to a number of PTEs corresponding to a given memory
    /// range.
    pub fn modify_range<F>(&mut self, _range: &MemoryRegion, _f: &F) -> Result<()>
    where
        F: Fn(&MemoryRegion, &mut Descriptor, usize) -> result::Result<(), ()>,
    {
        Ok(())
    }

    /// Applies the provided callback function to a number of PTEs corresponding to a given memory
    /// range.
    pub fn walk_range<F>(&self, _range: &MemoryRegion, _f: &F) -> Result<()>
    where
        F: Fn(&MemoryRegion, &Descriptor, usize) -> result::Result<(), ()>,
    {
        Ok(())
    }
}
