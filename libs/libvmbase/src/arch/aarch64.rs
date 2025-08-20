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

//! Wrappers of assembly calls.

pub mod bionic;
pub mod exceptions;
pub mod hvc;
pub mod layout;
pub mod page_table;
pub mod platform;
pub mod rand;
pub mod uart;

/// Reads a value from a system register.
#[macro_export]
macro_rules! read_sysreg {
    ($sysreg:literal) => {{
        let mut r: usize;
        #[allow(unused_unsafe)] // In case the macro is used within an unsafe block.
        // SAFETY: Reading a system register does not affect memory.
        unsafe {
            core::arch::asm!(
                concat!("mrs {}, ", $sysreg),
                out(reg) r,
                options(nomem, nostack, preserves_flags),
            )
        }
        r
    }};
}

/// Writes a value to a system register.
///
/// # Safety
///
/// Callers must ensure that side effects of updating the system register are properly handled.
#[macro_export]
macro_rules! write_sysreg {
    ($sysreg:literal, $val:expr) => {{
        let value: usize = $val;
        core::arch::asm!(
            concat!("msr ", $sysreg, ", {}"),
            in(reg) value,
            options(nomem, nostack, preserves_flags),
        )
    }};
}

/// Executes an instruction synchronization barrier.
#[macro_export]
macro_rules! isb {
    () => {{
        #[allow(unused_unsafe)] // In case the macro is used within an unsafe block.
        // SAFETY: memory barriers do not affect Rust's memory model.
        unsafe {
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        }
    }};
}

/// Executes a data synchronization barrier.
#[macro_export]
macro_rules! dsb {
    ($option:literal) => {{
        #[allow(unused_unsafe)] // In case the macro is used within an unsafe block.
        // SAFETY: memory barriers do not affect Rust's memory model.
        unsafe {
            core::arch::asm!(concat!("dsb ", $option), options(nomem, nostack, preserves_flags));
        }
    }};
}

/// Executes a data cache operation.
#[macro_export]
macro_rules! dc {
    ($option:literal, $addr:expr) => {{
        let addr: usize = $addr;
        #[allow(unused_unsafe)] // In case the macro is used within an unsafe block.
        // SAFETY: Clearing cache lines shouldn't have Rust-visible side effects.
        unsafe {
            core::arch::asm!(
                concat!("dc ", $option, ", {x}"),
                x = in(reg) addr,
                options(nomem, nostack, preserves_flags),
            );
        }
    }};
}

/// Invalidates cached leaf PTE entries by virtual address.
#[macro_export]
macro_rules! tlbi {
    ($option:literal, $asid:expr, $addr:expr) => {{
        let asid: usize = $asid;
        let addr: usize = $addr;
        #[allow(unused_unsafe)] // In case the macro is used within an unsafe block.
        // SAFETY: Invalidating the TLB doesn't affect Rust. When the address matches a
        // block entry larger than the page size, all translations for the block are invalidated.
        unsafe {
            core::arch::asm!(
                concat!("tlbi ", $option, ", {x}"),
                x = in(reg) (asid << 48) | (addr >> 12),
                options(nomem, nostack, preserves_flags)
            );
        }
    }};
}

/// Reads the number of words in the smallest cache line of all the data caches and unified caches.
#[inline]
pub fn min_dcache_line_size() -> usize {
    const DMINLINE_SHIFT: usize = 16;
    const DMINLINE_MASK: usize = 0xf;
    let ctr_el0 = read_sysreg!("ctr_el0");

    // DminLine: log2 of the number of words in the smallest cache line of all the data caches.
    let dminline = (ctr_el0 >> DMINLINE_SHIFT) & DMINLINE_MASK;

    1 << dminline
}

/// Reads the hardware dirty bit management flag (ID_AA64MMFR1_EL1.HAFDBS[1]).
#[inline]
pub fn id_aa64mmfr1_el1_hafdbs() -> bool {
    const ID_AA64MMFR1_EL1_HAFDBS: usize = 1 << 1;
    read_sysreg!("id_aa64mmfr1_el1") & ID_AA64MMFR1_EL1_HAFDBS != 0
}

/// Sets or clears TCR_EL1.{HA,HD} bits controlling hardware management of access and dirty state.
#[inline]
fn set_tcr_el1_ha_hd(enabled: bool) {
    const TCR_EL1_HA_HD_BITS: usize = 3 << 39;

    let mut tcr = read_sysreg!("tcr_el1");
    if enabled {
        tcr |= TCR_EL1_HA_HD_BITS;
    } else {
        tcr &= !TCR_EL1_HA_HD_BITS;
    }
    // SAFETY: Changing this bit in TCR doesn't affect Rust's view of memory.
    unsafe { write_sysreg!("tcr_el1", tcr) }
    isb!();
}
