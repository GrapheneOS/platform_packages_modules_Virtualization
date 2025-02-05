// Copyright 2023, The Android Open Source Project
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

//! Memory layout for crosvm for aarch64 architecture.
//!
//! https://crosvm.dev/book/appendix/memory_layout.html#common-layout

use crate::memory::page_4kb_of;
use core::ops::Range;
use static_assertions::const_assert_eq;

/// The start address of MMIO space.
pub const MMIO_START: usize = 0x0;
/// The end address of MMIO space.
pub const MMIO_END: usize = PVMFW_START;
/// MMIO range.
pub const MMIO_RANGE: Range<usize> = MMIO_START..MMIO_END;

/// Start pvmfw region.
pub const PVMFW_START: usize = 0x7fc00000;

/// The start of the system's contiguous "main" memory.
pub const MEM_START: usize = 0x8000_0000;

/// Size of the FDT region as defined by crosvm, both in kernel and BIOS modes.
pub const FDT_MAX_SIZE: usize = 2 << 20;

/// First address that can't be translated by a level 1 TTBR0_EL1.
pub const MAX_VIRT_ADDR: usize = 1 << 40;

/// Base memory-mapped addresses of the UART devices.
///
/// See SERIAL_ADDR in https://crosvm.dev/book/appendix/memory_layout.html#common-layout.
pub const UART_ADDRESSES: [usize; 4] = [0x3f8, 0x2f8, 0x3e8, 0x2e8];

/// Address of the single page containing all the UART devices.
pub const UART_PAGE_ADDR: usize = 0;
const_assert_eq!(UART_PAGE_ADDR, page_4kb_of(UART_ADDRESSES[0]));
const_assert_eq!(UART_PAGE_ADDR, page_4kb_of(UART_ADDRESSES[1]));
const_assert_eq!(UART_PAGE_ADDR, page_4kb_of(UART_ADDRESSES[2]));
const_assert_eq!(UART_PAGE_ADDR, page_4kb_of(UART_ADDRESSES[3]));
