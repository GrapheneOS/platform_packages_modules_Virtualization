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
.code64
.global entry
entry:
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

	/* Load global descriptor table from the data section. */
	lgdt gdtr(%rip)

	/* Reload all data segments with the data descriptor from the new GDT. */
	movw $DATA_SELECTOR, %ax
	movw %ax, %ss
	movw %ax, %ds
	movw %ax, %es
	movw %ax, %fs
	movw %ax, %gs

	/* RETF to CODE_SELECTOR:.Lreload_cs to switch CS to the new code descriptor. */
	leaq .Lreload_cs(%rip), %rax
	pushq $CODE_SELECTOR
	pushq %rax
	lretq
.Lreload_cs:

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
