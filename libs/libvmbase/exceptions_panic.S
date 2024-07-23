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

#include <common.h>

/**
 * The following table is intended to trap any fault resulting from the very
 * first memory accesses. They assume that PSCI v0.2 is available and provides
 * the PSCI_SYSTEM_RESET call in an attempt to gracefully exit but otherwise
 * results in the core busy-looping.
 */

.section .text.vector_table_panic, "ax"
.global vector_table_panic
.balign 0x800
vector_table_panic:
sync_cur_sp0_panic:
	reset_or_hang

.balign 0x80
irq_cur_sp0_panic:
	reset_or_hang

.balign 0x80
fiq_cur_sp0_panic:
	reset_or_hang

.balign 0x80
serr_cur_sp0_panic:
	reset_or_hang

.balign 0x80
sync_cur_spx_panic:
	reset_or_hang

.balign 0x80
irq_cur_spx_panic:
	reset_or_hang

.balign 0x80
fiq_cur_spx_panic:
	reset_or_hang

.balign 0x80
serr_cur_spx_panic:
	reset_or_hang

.balign 0x80
sync_lower_64_panic:
	reset_or_hang

.balign 0x80
irq_lower_64_panic:
	reset_or_hang

.balign 0x80
fiq_lower_64_panic:
	reset_or_hang

.balign 0x80
serr_lower_64_panic:
	reset_or_hang

.balign 0x80
sync_lower_32_panic:
	reset_or_hang

.balign 0x80
irq_lower_32_panic:
	reset_or_hang

.balign 0x80
fiq_lower_32_panic:
	reset_or_hang

.balign 0x80
serr_lower_32_panic:
	reset_or_hang
