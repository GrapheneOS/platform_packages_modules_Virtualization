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
