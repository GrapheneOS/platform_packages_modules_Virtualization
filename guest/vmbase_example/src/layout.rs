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

use aarch64_paging::paging::{MemoryRegion, VirtualAddress};
use core::ops::Range;
use log::info;
use vmbase::{layout, memory::PAGE_SIZE};

/// The first 1 GiB of memory are used for MMIO.
pub const DEVICE_REGION: MemoryRegion = MemoryRegion::new(0, 0x40000000);

/// Writable data region for the stack.
pub fn boot_stack_range() -> Range<VirtualAddress> {
    layout::stack_range(40 * PAGE_SIZE)
}

pub fn print_addresses() {
    let text = layout::text_range();
    info!("text:       {}..{} ({} bytes)", text.start, text.end, text.end - text.start);
    let rodata = layout::rodata_range();
    info!("rodata:     {}..{} ({} bytes)", rodata.start, rodata.end, rodata.end - rodata.start);
    info!("binary end: {}", layout::binary_end());
    let data = layout::data_range();
    info!(
        "data:       {}..{} ({} bytes, loaded at {})",
        data.start,
        data.end,
        data.end - data.start,
        layout::data_load_address(),
    );
    let bss = layout::bss_range();
    info!("bss:        {}..{} ({} bytes)", bss.start, bss.end, bss.end - bss.start);
    let boot_stack = boot_stack_range();
    info!(
        "boot_stack: {}..{} ({} bytes)",
        boot_stack.start,
        boot_stack.end,
        boot_stack.end - boot_stack.start
    );
}
