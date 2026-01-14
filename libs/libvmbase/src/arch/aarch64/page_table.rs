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

use crate::arch::aarch64::id_aa64mmfr1_el1_hafdbs;
use crate::arch::aarch64::set_tcr_el1_ha_hd;
use crate::arch::flush_region;
use crate::dsb;
use crate::isb;
use crate::mmu::{MmuError, MmuOps, MmuResult};
use crate::read_sysreg;
use crate::tlbi;
use aarch64_paging::idmap::IdMap;
use aarch64_paging::paging::{
    Attributes, Constraints, Descriptor, MemoryRegion, TranslationRegime, VirtualAddress,
};
use core::ops::Range;

/// Software bit used to indicate a device that should be lazily mapped.
pub const MMIO_LAZY_MAP_FLAG: Attributes = Attributes::SWFLAG_0;

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

/// High-level API for managing MMU mappings.
pub struct PageTable {
    idmap: IdMap,
    uses_hafdbs: bool,
}

impl From<IdMap> for PageTable {
    fn from(idmap: IdMap) -> Self {
        Self { idmap, uses_hafdbs: false }
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
}

impl MmuOps for PageTable {
    unsafe fn activate(&mut self) {
        self.uses_hafdbs = cfg!(feature = "cpu_feat_hafdbs") && id_aa64mmfr1_el1_hafdbs();
        // Activate dirty state management first, otherwise we may get permission faults
        // immediately after activating the new page table. This has no effect before the new page
        // table is activated because none of the entries in the initial idmap have the DBM flag.
        set_tcr_el1_ha_hd(self.uses_hafdbs);
        // SAFETY: the caller of this unsafe function asserts that switching to a different
        // translation is safe
        unsafe { self.idmap.activate() }
    }

    fn mark_as_lazy_device(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&as_memory_region(range), DEVICE_LAZY)?;
        Ok(())
    }

    fn map_device(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&as_memory_region(range), DEVICE)?;
        Ok(())
    }

    fn map_device_expect_lazy(&mut self, range: &Range<usize>) -> MmuResult<()> {
        let region = as_memory_region(range);
        // This must be safe and free from break-before-make (BBM) violations, given that the
        // initial lazy mapping has the valid bit cleared, and each newly created valid descriptor
        // created inside the mapping has the same size and alignment.
        self.idmap.modify_range(&region, &|_: &MemoryRegion, d: &mut Descriptor, _: usize| {
            let flags = d.flags().expect("Unsupported PTE flags set");
            if !flags.contains(MMIO_LAZY_MAP_FLAG) || flags.contains(Attributes::VALID) {
                return Err(());
            }
            d.modify_flags(Attributes::VALID, Attributes::empty());
            Ok(())
        })?;
        Ok(())
    }

    fn map_data(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&as_memory_region(range), DATA)?;
        Ok(())
    }

    fn map_data_track_dirty_state(&mut self, range: &Range<usize>) -> MmuResult<()> {
        if !self.uses_hafdbs {
            // TODO(b/472909113): Avoid SW DBM until data corruptions are fixed.
            return self.map_data(range);
        }
        let region = as_memory_region(range);
        // Map the region down to pages to minimize the size of the regions that will be marked
        // dirty once a store hits them, but also to ensure that we can clear the read-only
        // attribute while the mapping is live without causing break-before-make (BBM) violations.
        // The latter implies that we must avoid the use of the contiguous hint as well.
        self.idmap.map_range_with_constraints(
            &region,
            DATA_DBM,
            Constraints::NO_BLOCK_MAPPINGS | Constraints::NO_CONTIGUOUS_HINT,
        )?;
        Ok(())
    }

    fn map_code(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&as_memory_region(range), CODE)?;
        Ok(())
    }

    fn map_rodata(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&as_memory_region(range), RODATA)?;
        Ok(())
    }

    fn mark_data_dirty(&mut self, range: &Range<usize>) -> MmuResult<()> {
        let region = as_memory_region(range);
        self.idmap.modify_range(&region, &|r: &MemoryRegion, d: &mut Descriptor, _: usize| {
            let flags = d.flags().ok_or(())?;
            assert!(flags.contains(Attributes::READ_ONLY), "unexpected PTE writable state");
            if !flags.contains(Attributes::DBM) {
                return Err(());
            }
            d.modify_flags(Attributes::empty(), Attributes::READ_ONLY);
            // Updating the read-only bit of a PTE requires TLB invalidation.
            tlbi!("vale1", Self::ASID, r.start().0);
            // A TLB maintenance instruction is only guaranteed to be complete after a DSB
            // instruction.
            dsb!("ish");
            // An ISB instruction is required to ensure the effects of completed TLB maintenance
            // instructions are visible to instructions fetched afterwards.
            // See ARM ARM E2.3.10, and G5.9.
            isb!();
            Ok(())
        })?;
        Ok(())
    }

    // aarch64 doesn't support unmap for now.
    fn unmap(&mut self, range: &Range<usize>) -> MmuResult<()> {
        Err(MmuError::RegionLocked(range.clone()))
    }

    fn sync_dirty_state(&mut self) -> MmuResult<()> {
        // Execute a barrier instruction to ensure all hardware updates to the page table have been
        // observed before reading PTE flags to determine dirty state.
        dsb!("ish");
        Ok(())
    }

    fn flush_dirty_pages(&mut self, range: &Range<usize>) -> MmuResult<()> {
        let region = as_memory_region(range);
        self.idmap.walk_range(&region, &mut |r: &MemoryRegion, d: &Descriptor, _: usize| {
            let flags = d.flags().ok_or(())?;
            if !flags.contains(Attributes::READ_ONLY) {
                flush_region(r.start().0, r.len());
            }
            Ok(())
        })?;
        Ok(())
    }
}

impl Drop for PageTable {
    fn drop(&mut self) {
        if self.uses_hafdbs {
            set_tcr_el1_ha_hd(false);
        }
        // Dropping self.idmap sets TTBR_EL0 back to the static PTs.
    }
}

fn as_memory_region(range: &Range<usize>) -> MemoryRegion {
    (VirtualAddress(range.start)..VirtualAddress(range.end)).into()
}
