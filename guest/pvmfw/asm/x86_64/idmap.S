/*
 * Copyright 2025 The Android Open Source Project
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

.set .L_PRESENT, 1 << 0
.set .L_WRITABLE, 1 << 1
.set .L_USER, 1 << 2
.set .L_PAGE_WRITE_THROUGH, 1 << 3
.set .L_PAGE_CACHE_DISABLED, 1 << 4
.set .L_PAGE_SIZE, 1 << 7
.set .L_GLOBAL, 1 << 8
.set .L_EXECUTE_DISABLE, 1 << 63

/* Page map level 4 table entry flags */
.set .L_PML4E_PDPT, .L_PRESENT

/* Page directory pointer table entry flags */
.set .L_PDPTE_PD, .L_PRESENT

/* Page directory flags */
.set .L_PDE_2MB_RX, .L_PRESENT | .L_PAGE_SIZE
.set .L_PDE_2MB_RW, .L_PRESENT | .L_PAGE_SIZE | .L_WRITABLE | .L_EXECUTE_DISABLE

.section ".rodata.idmap", "a", %progbits

/* PML4 table (page global directory) */
.global idmap
.align 4096
idmap:
	/* 0x0000000000 - 0x8000000000 (0-512 GB) */
	.quad .L_PML4E_PDPT + .L_pdpt

	/* 0x8000000000+: not present */
	.balign 4096, 0


/* Page directory pointer table for 0 - 512 GB */
.align 4096
.L_pdpt:
	/* 0 - 1 GB: not present */
	.quad 0

	/* 1 - 2 GB */
	.quad .L_PDPTE_PD + .L_pd_1g

	/* 2+ GB: not present */
	.balign 4096, 0


/* Page directory for 1 GB - 2 GB */
.align 4096
.L_pd_1g:
	/* 0x40000000 - 0x7fc00000: not present */
	.fill 512-2, 8, 0

	/* 0x7fc00000 - 0x7fe00000: pVM firmware image */
	.quad .L_PDE_2MB_RX + 0x7fc00000

	/* 0x7fe00000 - 0x80000000: Writable memory for .bss and .data */
	.quad .L_PDE_2MB_RW + 0x7fe00000
