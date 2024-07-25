// Copyright 2022, The Android Open Source Project
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

use crate::read_sysreg;
use aarch64_paging::idmap::IdMap;
use aarch64_paging::paging::{
    Attributes, Constraints, Descriptor, MemoryRegion, TranslationRegime,
};
use aarch64_paging::MapError;
use core::result;

/// Software bit used to indicate a device that should be lazily mapped.
pub(super) const MMIO_LAZY_MAP_FLAG: Attributes = Attributes::SWFLAG_0;

/// We assume that MAIR_EL1.Attr0 = "Device-nGnRE memory" (0b0000_0100)
const DEVICE_NGNRE: Attributes = Attributes::ATTRIBUTE_INDEX_0;

/// We assume that MAIR_EL1.Attr1 = "Normal memory, Outer & Inner WB Non-transient, R/W-Allocate"
/// (0b1111_1111)
const NORMAL: Attributes = Attributes::ATTRIBUTE_INDEX_1.union(Attributes::INNER_SHAREABLE);

const MEMORY: Attributes =
    Attributes::VALID.union(NORMAL).union(Attributes::NON_GLOBAL).union(Attributes::ACCESSED);
const DEVICE_LAZY: Attributes =
    MMIO_LAZY_MAP_FLAG.union(DEVICE_NGNRE).union(Attributes::UXN).union(Attributes::ACCESSED);
const DEVICE: Attributes = DEVICE_LAZY.union(Attributes::VALID);
const CODE: Attributes = MEMORY.union(Attributes::READ_ONLY);
const DATA: Attributes = MEMORY.union(Attributes::UXN);
const RODATA: Attributes = DATA.union(Attributes::READ_ONLY);
const DATA_DBM: Attributes = RODATA.union(Attributes::DBM);

type Result<T> = result::Result<T, MapError>;

/// High-level API for managing MMU mappings.
pub struct PageTable {
    idmap: IdMap,
}

impl From<IdMap> for PageTable {
    fn from(idmap: IdMap) -> Self {
        Self { idmap }
    }
}

impl Default for PageTable {
    fn default() -> Self {
        const TCR_EL1_TG0_MASK: usize = 0x3;
        const TCR_EL1_TG0_SHIFT: u32 = 14;
        const TCR_EL1_TG0_SIZE_4KB: usize = 0b00;

        const TCR_EL1_T0SZ_MASK: usize = 0x3f;
        const TCR_EL1_T0SZ_SHIFT: u32 = 0;
        const TCR_EL1_T0SZ_39_VA_BITS: usize = 64 - 39;

        // Ensure that entry.S wasn't changed without updating the assumptions about TCR_EL1 here.
        let tcr_el1 = read_sysreg!("tcr_el1");
        assert_eq!((tcr_el1 >> TCR_EL1_TG0_SHIFT) & TCR_EL1_TG0_MASK, TCR_EL1_TG0_SIZE_4KB);
        assert_eq!((tcr_el1 >> TCR_EL1_T0SZ_SHIFT) & TCR_EL1_T0SZ_MASK, TCR_EL1_T0SZ_39_VA_BITS);

        IdMap::new(Self::ASID, Self::ROOT_LEVEL, TranslationRegime::El1And0).into()
    }
}

impl PageTable {
    /// ASID used for the underlying page table.
    pub const ASID: usize = 1;

    /// Level of the underlying page table's root page.
    const ROOT_LEVEL: usize = 1;

    /// Activates the page table.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the PageTable instance has valid and identical mappings for the
    /// code being currently executed. Otherwise, the Rust execution model (on which the borrow
    /// checker relies) would be violated.
    pub unsafe fn activate(&mut self) {
        // SAFETY: the caller of this unsafe function asserts that switching to a different
        // translation is safe
        unsafe { self.idmap.activate() }
    }

    /// Maps the given range of virtual addresses to the physical addresses as lazily mapped
    /// nGnRE device memory.
    pub fn map_device_lazy(&mut self, range: &MemoryRegion) -> Result<()> {
        self.idmap.map_range(range, DEVICE_LAZY)
    }

    /// Maps the given range of virtual addresses to the physical addresses as valid device
    /// nGnRE device memory.
    pub fn map_device(&mut self, range: &MemoryRegion) -> Result<()> {
        self.idmap.map_range(range, DEVICE)
    }

    /// Maps the given range of virtual addresses to the physical addresses as non-executable
    /// and writable normal memory.
    pub fn map_data(&mut self, range: &MemoryRegion) -> Result<()> {
        self.idmap.map_range(range, DATA)
    }

    /// Maps the given range of virtual addresses to the physical addresses as non-executable,
    /// read-only and writable-clean normal memory.
    pub fn map_data_dbm(&mut self, range: &MemoryRegion) -> Result<()> {
        // Map the region down to pages to minimize the size of the regions that will be marked
        // dirty once a store hits them, but also to ensure that we can clear the read-only
        // attribute while the mapping is live without causing break-before-make (BBM) violations.
        // The latter implies that we must avoid the use of the contiguous hint as well.
        self.idmap.map_range_with_constraints(
            range,
            DATA_DBM,
            Constraints::NO_BLOCK_MAPPINGS | Constraints::NO_CONTIGUOUS_HINT,
        )
    }

    /// Maps the given range of virtual addresses to the physical addresses as read-only
    /// normal memory.
    pub fn map_code(&mut self, range: &MemoryRegion) -> Result<()> {
        self.idmap.map_range(range, CODE)
    }

    /// Maps the given range of virtual addresses to the physical addresses as non-executable
    /// and read-only normal memory.
    pub fn map_rodata(&mut self, range: &MemoryRegion) -> Result<()> {
        self.idmap.map_range(range, RODATA)
    }

    /// Applies the provided updater function to a number of PTEs corresponding to a given memory
    /// range.
    pub fn modify_range<F>(&mut self, range: &MemoryRegion, f: &F) -> Result<()>
    where
        F: Fn(&MemoryRegion, &mut Descriptor, usize) -> result::Result<(), ()>,
    {
        self.idmap.modify_range(range, f)
    }

    /// Applies the provided callback function to a number of PTEs corresponding to a given memory
    /// range.
    pub fn walk_range<F>(&self, range: &MemoryRegion, f: &F) -> Result<()>
    where
        F: Fn(&MemoryRegion, &Descriptor, usize) -> result::Result<(), ()>,
    {
        let mut callback = |mr: &MemoryRegion, d: &Descriptor, l: usize| f(mr, d, l);
        self.idmap.walk_range(range, &mut callback)
    }
}
