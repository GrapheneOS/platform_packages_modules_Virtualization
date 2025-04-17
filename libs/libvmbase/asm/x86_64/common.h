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

#pragma once

/* Control Register 0 */
#define CR0_PE          0x00000001
#define CR0_MP          0x00000002
#define CR0_EM          0x00000004
#define CR0_TS          0x00000008
#define CR0_ET          0x00000010
#define CR0_NE          0x00000020
#define CR0_WP          0x00001000
#define CR0_AM          0x00004000
#define CR0_NW          0x20000000
#define CR0_CD          0x40000000
#define CR0_PG          0x80000000

/* Control Register 4 */
#define CR4_VME         0x00000001
#define CR4_PVI         0x00000002
#define CR4_TSD         0x00000004
#define CR4_DE          0x00000008
#define CR4_PSE         0x00000010
#define CR4_PAE         0x00000020
#define CR4_MCE         0x00000040
#define CR4_PGE         0x00000080
#define CR4_OSFXSR      0x00000100
#define CR4_OSXMMEXCPT  0x00000200
#define CR4_LA57        0x00000400
#define CR4_FSGSBASE    0x00010000
#define CR4_PCIDE       0x00020000
#define CR4_OSXSAVE     0x00040000
#define CR4_SMEP        0x00100000
#define CR4_SMAP        0x00200000
#define CR4_PKE         0x00400000
#define CR4_CET         0x00800000

/* Extended Control Register 0 */
#define XCR0_X87        0x00000001
#define XCR0_SSE        0x00000002
#define XCR0_AVX        0x00000004
#define XCR0_BNDREG     0x00000008
#define XCR0_BNDCSR     0x00000010
#define XCR0_opmask     0x00000020
#define XCR0_ZMM_Hi256  0x00000040
#define XCR0_Hi16_ZMM   0x00000080
#define XCR0_PKRU       0x00000200
#define XCR0_CET_U      0x00000800
#define XCR0_CET_S      0x00001000
#define XCR0_TILECFG    0x00020000
#define XCR0_TILEDATA   0x00040000

/* Extended Feature Enable Register */
#define EFER_LME        (1 << 8)
#define EFER_LMA        (1 << 10)
#define EFER_NXE        (1 << 11)

/* Architecturally defined MSRs */
#define IA32_EFER       0xC0000080
#define IA32_FS_BASE    0xC0000100

/*
 *  GDT Access Byte
 *
 *  7                               0
 *  +---+-----+---+---+----+----+---+
 *  | P | DPL | S | E | DC | RW | A |
 *  +---+-----+---+---+----+----+---+
 *                |                 |
 *                |<-     TYPE    ->|
 *
 *  P   1b - Present
 *  DPL 2b - Descriptor privilege level (ring)
 *  S   1b - System descriptor (0)
 *  E   1b - Executable
 *  DC  1b - Direction/Conforming
 *  RW  1b - Read/Write
 *  A   1b - Access (1)
 *
 */

#define GDT_ACCESS_P 0x80
#define GDT_ACCESS_S 0x10

/* Documentation reference
 * Intel Volume 3A - 3.4.5.1
 * Table 3-1 Code- and Data-Segment Types
 *
 * Access type ACCESS:
 * R - Read
 * W - Write
 * A - Accessed
 * E - Expand down
 * X - Executable
 * C - Conforming
 */
#define GDT_ACCESS_TYPE_DATA_RO 0x00
#define GDT_ACCESS_TYPE_DATA_ROA 0x01
#define GDT_ACCESS_TYPE_DATA_RW 0x02
#define GDT_ACCESS_TYPE_DATA_RWA 0x03
#define GDT_ACCESS_TYPE_DATA_ROE 0x04
#define GDT_ACCESS_TYPE_DATA_ROEA 0x05
#define GDT_ACCESS_TYPE_DATA_RWE 0x06
#define GDT_ACCESS_TYPE_DATA_RWEA 0x07
#define GDT_ACCESS_TYPE_CODE_XO 0x08
#define GDT_ACCESS_TYPE_CODE_XA 0x09
#define GDT_ACCESS_TYPE_CODE_XR 0x0a
#define GDT_ACCESS_TYPE_CODE_XRA 0x0b
#define GDT_ACCESS_TYPE_CODE_XOC 0x0c
#define GDT_ACCESS_TYPE_CODE_XOCA 0x0d
#define GDT_ACCESS_TYPE_CODE_XRC 0x0e
#define GDT_ACCESS_TYPE_CODE_XRCA 0x0f

/*
 *
 *  Flags bitfield
 *
 *  3                  0
 *  +----+----+---+-----+
 *  | G  | DB | L | AVL |
 *  +----+----+---+-----+
 *
 *  G   1b - Granularity
 *           (0) - 1 B
 *           (1) - 4 KiB
 *  DB  1b - Size flag
 *           (0) - 16bit mode
 *           (1) - 32bit protected mode
 *  L   1b - Long mode code segment (1)
 *  AVL 1b - Available to system sw
 *
 */

#define GDT_FLAG_G_4KB 0x80
#define GDT_FLAG_DB_32 0x40
#define GDT_FLAG_LONG 0x20

#define CODE_SELECTOR 0x10
#define DATA_SELECTOR 0x18

.macro reset_or_hang
        /* Pulse reset line with 8042 keyboard controller. */
        mov $0xFE, %al
        out %al, $0x64
999:    hlt
        jmp 999b
.endm
