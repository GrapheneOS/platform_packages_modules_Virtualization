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

//! Memory layout for crosvm for x86-64 architecture.
//!
//! <https://crosvm.dev/book/appendix/memory_layout.html>

use core::ops::Range;

use crate::arch::VirtualAddress;

/// The start address of MMIO space.
pub const MMIO_START: usize = 0xD000_0000;
/// The end address of MMIO space.
pub const MMIO_END: usize = 0xF400_0000;
/// MMIO range.
pub const MMIO_RANGE: Range<usize> = MMIO_START..MMIO_END;

/// The start of the system's contiguous "main" memory.
pub const MEM_START: usize = 0x0;

/// Size of the FDT region as defined by crosvm.
pub const FDT_MAX_SIZE: usize = 1 << 20;

/// First address past the end of RAM in the low 4 GB.
pub const MAX_VIRT_ADDR: usize = 0xD000_0000;

/// Base I/O port numbers of the standard PC UART devices (COM1-COM4) provided by crosvm.
pub const UART_PORTS: [u16; 4] = [0x3f8, 0x2f8, 0x3e8, 0x2e8];

/// Range of the page at UART - not present on x86.
pub fn console_uart_page() -> Option<Range<VirtualAddress>> {
    None
}
