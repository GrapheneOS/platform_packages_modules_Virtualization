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

/* Architecturally defined MSRs */
#define IA32_FS_BASE    0xC0000100

.macro reset_or_hang
        /* Pulse reset line with 8042 keyboard controller. */
        mov $0xFE, %al
        out %al, $0x64
999:    hlt
        jmp 999b
.endm
