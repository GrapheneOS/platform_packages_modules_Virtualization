/*
 * Copyright 2022 The Android Open Source Project
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

.set .L_TT_TYPE_BLOCK, 0x1
.set .L_TT_TYPE_PAGE,  0x3
.set .L_TT_TYPE_TABLE, 0x3

/* Access flag. */
.set .L_TT_AF, 0x1 << 10
/* Not global. */
.set .L_TT_NG, 0x1 << 11
.set .L_TT_RO, 0x2 << 6
.set .L_TT_XN, 0x3 << 53

.set .L_TT_MT_DEV, 0x0 << 2			// MAIR #0 (DEV_nGnRE)
.set .L_TT_MT_MEM, (0x1 << 2) | (0x3 << 8)	// MAIR #1 (MEM_WBWA), inner shareable

.set .L_BLOCK_RO,  .L_TT_TYPE_BLOCK | .L_TT_MT_MEM | .L_TT_AF | .L_TT_RO | .L_TT_XN
.set .L_BLOCK_DEV, .L_TT_TYPE_BLOCK | .L_TT_MT_DEV | .L_TT_AF | .L_TT_XN
.set .L_BLOCK_MEM, .L_TT_TYPE_BLOCK | .L_TT_MT_MEM | .L_TT_AF | .L_TT_XN | .L_TT_NG
.set .L_BLOCK_MEM_XIP, .L_TT_TYPE_BLOCK | .L_TT_MT_MEM | .L_TT_AF | .L_TT_NG | .L_TT_RO

.set .L_PT_SZ, (4 << 10)
.set .L_PTE_L1_SZ, (1 << 30)
.set .L_PTE_L2_SZ, (2 << 20)

#if defined(VMBASE_IMAGE_LOAD_ADDR)
.set .L_OFFSET_L1, (VMBASE_IMAGE_LOAD_ADDR) / .L_PTE_L1_SZ
.set .L_OFFSET_L2, (VMBASE_IMAGE_LOAD_ADDR) % .L_PTE_L1_SZ / .L_PTE_L2_SZ
.set .L_XIP_ADDR, (VMBASE_IMAGE_LOAD_ADDR)
.set .L_MEM_ADDR, (VMBASE_IMAGE_LOAD_ADDR) + .L_PTE_L2_SZ
#else
#error "VMBASE_IMAGE_LOAD_ADDR undefined"
#endif

.section ".rodata.idmap", "a", %progbits
.global idmap
.align 12
idmap:
	/* level 1 */
	.quad		.L_BLOCK_DEV | 0x0		// 1 GB of device mappings
	.fill		.L_OFFSET_L1 - 1, 8, 0x0
	.quad		.L_TT_TYPE_TABLE + 0f
	.balign		.L_PT_SZ, 0x0

0:	/* level 2 */
	.fill		.L_OFFSET_L2, 8, 0x0
	.quad		.L_BLOCK_MEM_XIP | .L_XIP_ADDR
	.quad		.L_BLOCK_MEM	 | .L_MEM_ADDR
	.balign		.L_PT_SZ, 0x0
