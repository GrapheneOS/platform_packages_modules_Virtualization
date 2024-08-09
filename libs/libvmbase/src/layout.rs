// Copyright 2022, The Android Open Source Project
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

//! Memory layout.

pub mod crosvm;

use crate::linker::__stack_chk_guard;
use crate::memory::{page_4kb_of, PAGE_SIZE};
use aarch64_paging::paging::VirtualAddress;
use core::ops::Range;
use core::ptr::addr_of;
use static_assertions::const_assert_eq;

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

/// Get an address from a linker-defined symbol.
#[macro_export]
macro_rules! linker_addr {
    ($symbol:ident) => {{
        // SAFETY: We're just getting the address of an extern static symbol provided by the linker,
        // not dereferencing it.
        let addr = unsafe { addr_of!($crate::linker::$symbol) as usize };
        VirtualAddress(addr)
    }};
}

/// Gets the virtual address range between a pair of linker-defined symbols.
#[macro_export]
macro_rules! linker_region {
    ($begin:ident,$end:ident) => {{
        let start = linker_addr!($begin);
        let end = linker_addr!($end);

        start..end
    }};
}

/// Executable code.
pub fn text_range() -> Range<VirtualAddress> {
    linker_region!(text_begin, text_end)
}

/// Read-only data.
pub fn rodata_range() -> Range<VirtualAddress> {
    linker_region!(rodata_begin, rodata_end)
}

/// Initialised writable data.
pub fn data_range() -> Range<VirtualAddress> {
    linker_region!(data_begin, data_end)
}

/// Zero-initialized writable data.
pub fn bss_range() -> Range<VirtualAddress> {
    linker_region!(bss_begin, bss_end)
}

/// Writable data region for the stack.
pub fn stack_range(stack_size: usize) -> Range<VirtualAddress> {
    let end = linker_addr!(init_stack_pointer);
    let start = VirtualAddress(end.0.checked_sub(stack_size).unwrap());
    assert!(start >= linker_addr!(stack_limit));

    start..end
}

/// All writable sections, excluding the stack.
pub fn scratch_range() -> Range<VirtualAddress> {
    linker_region!(eh_stack_limit, bss_end)
}

/// Range of the page at UART_PAGE_ADDR of PAGE_SIZE.
pub fn console_uart_page() -> Range<VirtualAddress> {
    VirtualAddress(UART_PAGE_ADDR)..VirtualAddress(UART_PAGE_ADDR + PAGE_SIZE)
}

/// Read-write data (original).
pub fn data_load_address() -> VirtualAddress {
    linker_addr!(data_lma)
}

/// End of the binary image.
pub fn binary_end() -> VirtualAddress {
    linker_addr!(bin_end)
}

/// Value of __stack_chk_guard.
pub fn stack_chk_guard() -> u64 {
    // SAFETY: __stack_chk_guard shouldn't have any mutable aliases unless the stack overflows. If
    // it does, then there could be undefined behaviour all over the program, but we want to at
    // least have a chance at catching it.
    unsafe { addr_of!(__stack_chk_guard).read_volatile() }
}
