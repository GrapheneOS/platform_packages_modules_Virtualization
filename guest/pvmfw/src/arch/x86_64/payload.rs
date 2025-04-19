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

//! x86_64 low-level payload entry point

use core::arch::asm;

use vmbase::{layout, memory::deactivate_dynamic_page_tables, util::RangeExt};

use crate::memory::MemorySlices;

/// Function boot payload after cleaning all secret from pvmfw memory
pub fn jump_to_payload(entrypoint: usize, slices: &MemorySlices) -> ! {
    // TODO(b/393354256): replace hard-coded crosvm layout with address from `slices`.
    let boot_params = 0x7000;

    let preserved_memory = slices.preserved_memory.map(|slice| {
        let r = slice.as_ptr_range();
        (r.start as usize)..(r.end as usize)
    });

    deactivate_dynamic_page_tables();

    const CR0_PE: u64 = 0x00000001;
    const CR0_ET: u64 = 0x00000010;
    const CR0_CD: u64 = 0x40000000;
    const CR0_PG: u64 = 0x80000000;
    const CR0_VAL: u64 = CR0_PE | CR0_ET | CR0_CD | CR0_PG;

    const CR4_OSFXSR: u64 = 0x00000100;
    const CR4_OSXMMEXCPT: u64 = 0x00000200;
    const CR4_OSXSAVE: u64 = 0x00040000;
    const CR4_MASK: u64 = !(CR4_OSFXSR | CR4_OSXMMEXCPT | CR4_OSXSAVE);

    const XCR0_X87: u64 = 0x00000001;
    const XCR0_VAL: u64 = XCR0_X87;

    let scratch = layout::data_bss_range();
    let stack = layout::stack_range();
    let eh_stack = layout::eh_stack_range();

    // A sub-region of the scratch memory might contain data for the next stage so skip zeroing it.
    // Alternatively, an empty region at the start of the scratch region is compatible with the ASM
    // implementation and results in the whole scratch region being zeroed.
    let skipped = preserved_memory.unwrap_or(scratch.start.0..scratch.start.0);
    assert!(skipped.is_within(&(scratch.start.0..scratch.end.0)));

    // Zero all memory that could hold secrets and that can't be safely written to from Rust.
    // Reset all registers and then jump to the payload at the given address, passing it the given
    // boot_params pointer.
    //
    // SAFETY: We're exiting pvmfw by passing the register values we need to a noreturn asm!().
    unsafe {
        asm!(
            // Zero .data & .bss until the start of the skipped region.
            "mov rdi, {scratch}",
            "mov rcx, {scratch_before_skipped}",
            "rep stosb",

            // Skip the skipped region and keep zeroing .data & .bss.
            "mov rdi, {skipped_end}",
            "mov rcx, {scratch_after_skipped}",
            "rep stosb",

            // Zero stack region.
            "mov rdi, {stack}",
            "mov rcx, {stack_size}",
            "rep stosb",

            // Zero EH stack region.
            "mov rdi, {eh_stack}",
            "mov rcx, {eh_stack_size}",
            "rep stosb",

            // 64-bit code should not normally use this, but clear x87 FPU state just to be sure.
            "fninit",

            // Clear the low 128 bits of the SSE/AVX registers.
            "xorps xmm0, xmm0",
            "xorps xmm1, xmm1",
            "xorps xmm2, xmm2",
            "xorps xmm3, xmm3",
            "xorps xmm4, xmm4",
            "xorps xmm5, xmm5",
            "xorps xmm6, xmm6",
            "xorps xmm7, xmm7",
            "xorps xmm8, xmm8",
            "xorps xmm9, xmm9",
            "xorps xmm10, xmm10",
            "xorps xmm11, xmm11",
            "xorps xmm12, xmm12",
            "xorps xmm13, xmm13",
            "xorps xmm14, xmm14",
            "xorps xmm15, xmm15",

            // Clear the upper bits of the AVX registers.
            // TODO: skip this based on CPUID if support for non-AVX hardware is ever added.
            "vzeroupper",

            // Reset control registers.
            "xor rcx, rcx",
            "mov rax, {xcr0_val}",
            "mov rdx, rax",
            "shr rdx, 32",
            "xsetbv",

            "mov rax, cr4",
            "and rax, {cr4_mask}",
            "mov cr4, rax",

            "mov rax, {cr0_val}",
            "mov cr0, rax",

            // Flush all caches after setting CR0.CD.
            "wbinvd",

            // Clear the general-purpose registers.
            // NOTE: r15 and rsi must be preserved until the final jump!
            "xor rax, rax",
            "xor rbx, rbx",
            "xor rcx, rcx",
            "xor rdx, rdx",
            "xor rdi, rdi",
            "xor rbp, rbp",
            "xor rsp, rsp",
            "xor  r8,  r8",
            "xor  r9,  r9",
            "xor r10, r10",
            "xor r11, r11",
            "xor r12, r12",
            "xor r13, r13",
            "xor r14, r14",

            // Pass the boot_params pointer to the payload in rsi.
            "jmp r15",

            // RAX, RDX, RCX, and RDI have special meanings for certain instructions (`REP STOSB`
            // and `XSETBV`), so prevent those registers from being allocated to other inputs.
            in("rax") 0, in("rcx") 0, in("rdx") 0, in("rdi") 0,

            scratch = in(reg) u64::try_from(scratch.start.0).unwrap(),
            scratch_before_skipped = in(reg) u64::try_from(skipped.start - scratch.start.0).unwrap(),
            scratch_after_skipped = in(reg) u64::try_from(scratch.end.0 - skipped.end).unwrap(),
            skipped_end = in(reg) u64::try_from(skipped.end).unwrap(),
            stack = in(reg) u64::try_from(stack.start.0).unwrap(),
            stack_size = in(reg) u64::try_from(stack.end.0 - stack.start.0).unwrap(),
            eh_stack = in(reg) u64::try_from(eh_stack.start.0).unwrap(),
            eh_stack_size = in(reg) u64::try_from(eh_stack.end.0 - eh_stack.start.0).unwrap(),
            in("rsi") u64::try_from(boot_params).unwrap(),
            in("r15") u64::try_from(entrypoint).unwrap(),

            cr0_val = const CR0_VAL,
            cr4_mask = const CR4_MASK,
            xcr0_val = const XCR0_VAL,

            options(noreturn),
        );
    };
}
