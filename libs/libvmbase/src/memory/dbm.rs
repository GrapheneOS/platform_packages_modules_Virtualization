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

//! Hardware management of the access flag and dirty state.

use super::page_table::PageTable;
use super::util::flush_region;
use crate::{dsb, isb, read_sysreg, tlbi, write_sysreg};
use aarch64_paging::paging::{Attributes, Descriptor, MemoryRegion};

/// Sets whether the hardware management of access and dirty state is enabled with
/// the given boolean.
pub(super) fn set_dbm_enabled(enabled: bool) {
    if !dbm_available() {
        return;
    }
    // TCR_EL1.{HA,HD} bits controlling hardware management of access and dirty state
    const TCR_EL1_HA_HD_BITS: usize = 3 << 39;

    let mut tcr = read_sysreg!("tcr_el1");
    if enabled {
        tcr |= TCR_EL1_HA_HD_BITS
    } else {
        tcr &= !TCR_EL1_HA_HD_BITS
    };
    // SAFETY: Changing this bit in TCR doesn't affect Rust's view of memory.
    unsafe { write_sysreg!("tcr_el1", tcr) }
    isb!();
}

/// Returns `true` if hardware dirty state management is available.
fn dbm_available() -> bool {
    if !cfg!(feature = "cpu_feat_hafdbs") {
        return false;
    }
    // Hardware dirty bit management available flag (ID_AA64MMFR1_EL1.HAFDBS[1])
    const DBM_AVAILABLE: usize = 1 << 1;
    read_sysreg!("id_aa64mmfr1_el1") & DBM_AVAILABLE != 0
}

/// Flushes a memory range the descriptor refers to, if the descriptor is in writable-dirty state.
pub(super) fn flush_dirty_range(
    va_range: &MemoryRegion,
    desc: &Descriptor,
    _level: usize,
) -> Result<(), ()> {
    let flags = desc.flags().ok_or(())?;
    if !flags.contains(Attributes::READ_ONLY) {
        flush_region(va_range.start().0, va_range.len());
    }
    Ok(())
}

/// Clears read-only flag on a PTE, making it writable-dirty. Used when dirty state is managed
/// in software to handle permission faults on read-only descriptors.
pub(super) fn mark_dirty_block(
    va_range: &MemoryRegion,
    desc: &mut Descriptor,
    _level: usize,
) -> Result<(), ()> {
    let flags = desc.flags().ok_or(())?;
    if flags.contains(Attributes::DBM) {
        assert!(flags.contains(Attributes::READ_ONLY), "unexpected PTE writable state");
        desc.modify_flags(Attributes::empty(), Attributes::READ_ONLY);
        // Updating the read-only bit of a PTE requires TLB invalidation.
        // A TLB maintenance instruction is only guaranteed to be complete after a DSB instruction.
        // An ISB instruction is required to ensure the effects of completed TLB maintenance
        // instructions are visible to instructions fetched afterwards.
        // See ARM ARM E2.3.10, and G5.9.
        tlbi!("vale1", PageTable::ASID, va_range.start().0);
        dsb!("ish");
        isb!();
        Ok(())
    } else {
        Err(())
    }
}
