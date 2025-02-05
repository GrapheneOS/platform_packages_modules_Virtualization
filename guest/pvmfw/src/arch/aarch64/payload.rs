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

//! ARM64 low-level payload entry point

use crate::memory::MemorySlices;
use core::arch::asm;
use core::mem::size_of;
use vmbase::util::RangeExt as _;
use vmbase::{arch::aarch64::min_dcache_line_size, layout, memory::deactivate_dynamic_page_tables};

/// Function boot payload after cleaning all secret from pvmfw memory
pub fn jump_to_payload(entrypoint: usize, slices: &MemorySlices) -> ! {
    let fdt_address = slices.fdt.as_ptr() as usize;
    let bcc = slices
        .dice_chain
        .map(|slice| {
            let r = slice.as_ptr_range();
            (r.start as usize)..(r.end as usize)
        })
        .expect("Missing DICE chain");

    deactivate_dynamic_page_tables();

    const ASM_STP_ALIGN: usize = size_of::<u64>() * 2;
    const SCTLR_EL1_RES1: u64 = (0b11 << 28) | (0b101 << 20) | (0b1 << 11);
    // Stage 1 instruction access cacheability is unaffected.
    const SCTLR_EL1_I: u64 = 0b1 << 12;
    // SETEND instruction disabled at EL0 in aarch32 mode.
    const SCTLR_EL1_SED: u64 = 0b1 << 8;
    // Various IT instructions are disabled at EL0 in aarch32 mode.
    const SCTLR_EL1_ITD: u64 = 0b1 << 7;

    const SCTLR_EL1_VAL: u64 = SCTLR_EL1_RES1 | SCTLR_EL1_ITD | SCTLR_EL1_SED | SCTLR_EL1_I;

    let scratch = layout::data_bss_range();

    assert_ne!(scratch.end - scratch.start, 0, "scratch memory is empty.");
    assert_eq!(scratch.start.0 % ASM_STP_ALIGN, 0, "scratch memory is misaligned.");
    assert_eq!(scratch.end.0 % ASM_STP_ALIGN, 0, "scratch memory is misaligned.");

    assert!(bcc.is_within(&(scratch.start.0..scratch.end.0)));
    assert_eq!(bcc.start % ASM_STP_ALIGN, 0, "Misaligned guest BCC.");
    assert_eq!(bcc.end % ASM_STP_ALIGN, 0, "Misaligned guest BCC.");

    let stack = layout::stack_range();

    assert_ne!(stack.end - stack.start, 0, "stack region is empty.");
    assert_eq!(stack.start.0 % ASM_STP_ALIGN, 0, "Misaligned stack region.");
    assert_eq!(stack.end.0 % ASM_STP_ALIGN, 0, "Misaligned stack region.");

    let eh_stack = layout::eh_stack_range();

    assert_ne!(eh_stack.end - eh_stack.start, 0, "EH stack region is empty.");
    assert_eq!(eh_stack.start.0 % ASM_STP_ALIGN, 0, "Misaligned EH stack region.");
    assert_eq!(eh_stack.end.0 % ASM_STP_ALIGN, 0, "Misaligned EH stack region.");

    // Zero all memory that could hold secrets and that can't be safely written to from Rust.
    // Disable the exception vector, caches and page table and then jump to the payload at the
    // given address, passing it the given FDT pointer.
    //
    // SAFETY: We're exiting pvmfw by passing the register values we need to a noreturn asm!().
    unsafe {
        asm!(
            "cmp {scratch}, {bcc}",
            "b.hs 1f",

            // Zero .data & .bss until BCC.
            "0: stp xzr, xzr, [{scratch}], 16",
            "cmp {scratch}, {bcc}",
            "b.lo 0b",

            "1:",
            // Skip BCC.
            "mov {scratch}, {bcc_end}",
            "cmp {scratch}, {scratch_end}",
            "b.hs 1f",

            // Keep zeroing .data & .bss.
            "0: stp xzr, xzr, [{scratch}], 16",
            "cmp {scratch}, {scratch_end}",
            "b.lo 0b",

            "1:",
            // Flush d-cache over .data & .bss (including BCC).
            "0: dc cvau, {cache_line}",
            "add {cache_line}, {cache_line}, {dcache_line_size}",
            "cmp {cache_line}, {scratch_end}",
            "b.lo 0b",

            "mov {cache_line}, {stack}",
            // Zero stack region.
            "0: stp xzr, xzr, [{stack}], 16",
            "cmp {stack}, {stack_end}",
            "b.lo 0b",

            // Flush d-cache over stack region.
            "0: dc cvau, {cache_line}",
            "add {cache_line}, {cache_line}, {dcache_line_size}",
            "cmp {cache_line}, {stack_end}",
            "b.lo 0b",

            "mov {cache_line}, {eh_stack}",
            // Zero EH stack region.
            "0: stp xzr, xzr, [{eh_stack}], 16",
            "cmp {eh_stack}, {eh_stack_end}",
            "b.lo 0b",

            // Flush d-cache over EH stack region.
            "0: dc cvau, {cache_line}",
            "add {cache_line}, {cache_line}, {dcache_line_size}",
            "cmp {cache_line}, {eh_stack_end}",
            "b.lo 0b",

            "msr sctlr_el1, {sctlr_el1_val}",
            "isb",
            "mov x1, xzr",
            "mov x2, xzr",
            "mov x3, xzr",
            "mov x4, xzr",
            "mov x5, xzr",
            "mov x6, xzr",
            "mov x7, xzr",
            "mov x8, xzr",
            "mov x9, xzr",
            "mov x10, xzr",
            "mov x11, xzr",
            "mov x12, xzr",
            "mov x13, xzr",
            "mov x14, xzr",
            "mov x15, xzr",
            "mov x16, xzr",
            "mov x17, xzr",
            "mov x18, xzr",
            "mov x19, xzr",
            "mov x20, xzr",
            "mov x21, xzr",
            "mov x22, xzr",
            "mov x23, xzr",
            "mov x24, xzr",
            "mov x25, xzr",
            "mov x26, xzr",
            "mov x27, xzr",
            "mov x28, xzr",
            "mov x29, xzr",
            "msr ttbr0_el1, xzr",
            // Ensure that CMOs have completed before entering payload.
            "dsb nsh",
            "br x30",
            sctlr_el1_val = in(reg) SCTLR_EL1_VAL,
            bcc = in(reg) u64::try_from(bcc.start).unwrap(),
            bcc_end = in(reg) u64::try_from(bcc.end).unwrap(),
            cache_line = in(reg) u64::try_from(scratch.start.0).unwrap(),
            scratch = in(reg) u64::try_from(scratch.start.0).unwrap(),
            scratch_end = in(reg) u64::try_from(scratch.end.0).unwrap(),
            stack = in(reg) u64::try_from(stack.start.0).unwrap(),
            stack_end = in(reg) u64::try_from(stack.end.0).unwrap(),
            eh_stack = in(reg) u64::try_from(eh_stack.start.0).unwrap(),
            eh_stack_end = in(reg) u64::try_from(eh_stack.end.0).unwrap(),
            dcache_line_size = in(reg) u64::try_from(min_dcache_line_size()).unwrap(),
            in("x0") u64::try_from(fdt_address).unwrap(),
            in("x30") u64::try_from(entrypoint).unwrap(),
            options(noreturn),
        );
    };
}
