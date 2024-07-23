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

//! Low-level allocation and tracking of main memory.

use crate::helpers::PVMFW_PAGE_SIZE;
use aarch64_paging::paging::VirtualAddress;
use aarch64_paging::MapError;
use core::ops::Range;
use core::result;
use log::error;
use vmbase::{
    layout,
    memory::{PageTable, SIZE_2MB, SIZE_4KB},
    util::align_up,
};

/// Returns memory range reserved for the appended payload.
pub fn appended_payload_range() -> Range<VirtualAddress> {
    let start = align_up(layout::binary_end().0, SIZE_4KB).unwrap();
    // pvmfw is contained in a 2MiB region so the payload can't be larger than the 2MiB alignment.
    let end = align_up(start, SIZE_2MB).unwrap();
    VirtualAddress(start)..VirtualAddress(end)
}

/// Region allocated for the stack.
pub fn stack_range() -> Range<VirtualAddress> {
    const STACK_PAGES: usize = 12;

    layout::stack_range(STACK_PAGES * PVMFW_PAGE_SIZE)
}

pub fn init_page_table() -> result::Result<PageTable, MapError> {
    let mut page_table = PageTable::default();

    // Stack and scratch ranges are explicitly zeroed and flushed before jumping to payload,
    // so dirty state management can be omitted.
    page_table.map_data(&layout::scratch_range().into())?;
    page_table.map_data(&stack_range().into())?;
    page_table.map_code(&layout::text_range().into())?;
    page_table.map_rodata(&layout::rodata_range().into())?;
    page_table.map_data_dbm(&appended_payload_range().into())?;
    if let Err(e) = page_table.map_device(&layout::console_uart_page().into()) {
        error!("Failed to remap the UART as a dynamic page table entry: {e}");
        return Err(e);
    }
    Ok(page_table)
}
