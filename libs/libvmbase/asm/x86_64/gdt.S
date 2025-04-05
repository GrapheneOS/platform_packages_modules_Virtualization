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

.section .rodata

/*
 * The GDT is laid out to match the Linux/x86 boot protocol, which requires CS=0x10 and DS=0x18.
 *
 * GDT descriptors:
 * - 0x00 - null descriptor (architecturally required)
 * - 0x08 - null descriptor (unused)
 * - 0x10 - code
 * - 0x18 - data
 *
 * Descriptor entry layout:
 * word  - limit  [16b]
 * word  - base_0 [16b]
 * byte  - base_1 [8b]
 * byte  - access  [8b]
 * byte  - flags  [4b] + limit [4b]
 * byte  - base_2 [8b]
 */
.align 16
.global gdt_table
gdt_table:

	/*
	 * 0x00: Null
	 * Base  0x00000
	 * Limit --
	 * Access --
	 */
	.word 0x0000
	.word 0x0000
	.byte 0x00
	.byte 0x00
	.byte 0x00
	.byte 0x00

	/*
	 * 0x08: Null
	 * Base  0x00000
	 * Limit --
	 * Access --
	 */
	.word 0x0000
	.word 0x0000
	.byte 0x00
	.byte 0x00
	.byte 0x00
	.byte 0x00

	/*
	 * 0x10: Code descriptor
	 * Base  0x00000
	 * Limit 0xfffff * 4KiB
	 * Access read + execute
	 */
	.word 0xffff
	.word 0x0000
	.byte 0x00
	.byte (GDT_ACCESS_P | GDT_ACCESS_S | GDT_ACCESS_TYPE_CODE_XRA)
	.byte (GDT_FLAG_G_4KB | GDT_FLAG_LONG | 0x0f)
	.byte 0x00

	/*
	 * 0x18: Data descriptor
	 * Base  0x00000
	 * Limit 0xfffff * 4KiB
	 * Access read + write
	 */
	.word 0xffff
	.word 0x0000
	.byte 0x00
	.byte (GDT_ACCESS_P | GDT_ACCESS_S | GDT_ACCESS_TYPE_DATA_RWA)
	.byte (GDT_FLAG_G_4KB | GDT_FLAG_DB_32 | 0x0f)
	.byte 0x00

gdt_table_end:

.align 8
.global gdtr
gdtr:
	/* size - 1 */
	.word gdt_table_end - gdt_table - 1
	/* offset */
	.quad gdt_table
