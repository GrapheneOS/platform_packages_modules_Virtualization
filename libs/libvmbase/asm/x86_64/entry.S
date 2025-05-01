/*
 * Copyright 2024 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#include "common.h"

.section .init.entry, "ax"
.code32
.global entry
entry:
	/* NOTE: The first few instructions must be valid opcodes in both 32- and 64-bit mode! */

	/* If paging is enabled, the CPU may be in long mode; check IA32_EFER.LMA. */
	movl %cr0, %eax
	testl $CR0_PG, %eax
	jnz .Lcheck_efer_lma

	/*
	 * Since paging is disabled, the CPU is definitely in 32-bit flat mode, not long mode or
	 * compatibility mode.
	 */

	/* Load our GDT. */
	movl $gdtr, %eax
	lgdt (%eax)

	/* Reload all data segment descriptors from our GDT. */
	movw $DATA_SELECTOR, %ax
	movw %ax, %ss
	movw %ax, %ds
	movw %ax, %es
	movw %ax, %fs
	movw %ax, %es

	/* Set page table pointer to static identity mapping. */
	movl $idmap, %eax
	movl %eax, %cr3

	/* Enable PAE. */
	movl %cr4, %eax
	orl $CR4_PAE, %eax
	movl %eax, %cr4

	/* Enable long mode and NX support. */
	movl %edx, %ebx
	movl $IA32_EFER, %ecx
	rdmsr
	orl $(EFER_LME | EFER_NXE), %eax
	wrmsr
	mov %ebx, %edx

	/* Enable paging. */
	movl %cr0, %eax
	orl $CR0_PG, %eax
	movl %eax, %cr0

	/* Far jump to reload CS and enter 64-bit long mode. */
	ljmp $CODE_SELECTOR, $.Lentry_64

.code64
.Lentry_64:

	/* Our load address is set by the host, so validate it before proceeding. */
	leaq entry(%rip), %rax
	movabs $entry, %rbx
	cmp %rax, %rbx
	je .Lload_address_ok
	reset_or_hang
.Lload_address_ok:

	/* Set stack pointer. */
	leaq init_stack_pointer(%rip), %rsp
	movq %rsp, %rbp

	/* Save boot parameters. */
	mov %rdi, %r12
	mov %rsi, %r13
	mov %rdx, %r14
	mov %rcx, %r15

	/* Zero BSS section. */
	leaq bss_begin(%rip), %rdi
	mov $bss_size, %rcx
	xor %eax, %eax
	cld
	rep stosb

	/* Relocate data section from data_lma to data_begin. */
	leaq data_lma(%rip), %rsi
	leaq data_begin(%rip), %rdi
	mov $data_size, %rcx

	cmp %rsi, %rdi
	je .Lno_reloc

	/* Copy top down to avoid overwriting data. */
	lea -1(%rsi,%rcx), %rsi
	lea -1(%rdi,%rcx), %rdi
	std
	rep movsb
	cld
.Lno_reloc:

	/* Initialize FPU (per Intel SDM, 10.2.1 "Configuring the x87 FPU Environment"). */
	mov %cr0, %rax
	and $~CR0_EM, %eax
	or $(CR0_MP | CR0_NE), %eax
	mov %rax, %cr0

	mov %cr4, %rax
	or $(CR4_OSFXSR | CR4_OSXMMEXCPT | CR4_OSXSAVE), %eax
	mov %rax, %cr4

	xor %ecx, %ecx /* XCR0 */
	xgetbv
	or $(XCR0_X87 | XCR0_SSE | XCR0_AVX), %eax
	xsetbv

	/* Point FS base at the TLS region. */
	leaq __bionic_tls(%rip), %rax
	mov %rax, (%rax) /* TLS_SLOT_SELF (slot 0) -> __bionic_tls */
	mov %rax, %rdx
	shr $32, %rdx
	mov $IA32_FS_BASE, %ecx
	wrmsr

	/* Call into Rust code entry point. */
	mov %r12, %rdi
	mov %r13, %rsi
	mov %r14, %rdx
	mov %r15, %rcx
	call rust_entry

	reset_or_hang


.code32
.Lcheck_efer_lma:
	/*
	 * If IA32_EFER.LMA is set, assume the CPU is in 64-bit mode (not compatibility mode), as
	 * there is no way to read the cached CS selector to determine if the L bit is set, and
	 * compatibility mode is not an expected initial state.
	 */
	movl %edx, %ebx
	movl $IA32_EFER, %ecx
	rdmsr
	test $EFER_LMA, %eax
	jnz .Lefer_lma_set

	/* Halt if paging is enabled but long mode is not active; this is not an expected state. */
	reset_or_hang

.code64
.Lefer_lma_set:

	/* Enable NX support in IA32_EFER. */
	orl $EFER_NXE, %eax
	wrmsr
	movl %ebx, %edx

	/* Load our GDT. */
	lgdt gdtr(%rip)

	/* Load all data segments with the data descriptor from our GDT. */
	movw $DATA_SELECTOR, %ax
	movw %ax, %ss
	movw %ax, %ds
	movw %ax, %es
	movw %ax, %fs
	movw %ax, %gs

	/* Set page table pointer to static identity mapping. */
	leaq idmap(%rip), %rax
	mov %rax, %cr3

	/* RETF to CODE_SELECTOR:.Lentry_64 to switch CS to the new code descriptor. */
	leaq init_stack_pointer(%rip), %rsp
	leaq .Lentry_64(%rip), %rax
	pushq $CODE_SELECTOR
	pushq %rax
	lretq
