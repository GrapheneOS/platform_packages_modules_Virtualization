// Copyright 2025, The Android Open Source Project
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

//! Main kernel source file of the VM used in testing of the low-level functionality.
//! For more information see ../README.md.

#![no_main]
#![no_std]

mod error;

use crate::error::Result;
use core::num::NonZeroUsize;
use core::slice;
use log::{error, info};
use vmbase::layout::crosvm;
use vmbase::memory::{map_rodata, resize_available_memory, SIZE_128KB};
use vmbase::power::reboot;
use vmbase::{configure_heap, generate_image_header, main};

/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
/// * The `fdt_addr` must be a valid pointer and points to a valid `Fdt`.
unsafe fn try_main(fdt_addr: usize) -> Result<()> {
    info!("Welcome to test VM!");

    let fdt_size = NonZeroUsize::new(crosvm::FDT_MAX_SIZE).unwrap();
    map_rodata(fdt_addr, fdt_size)?;
    // SAFETY: The tracker validated the range to be in main memory, mapped, and not overlap.
    let fdt = unsafe { slice::from_raw_parts(fdt_addr as *mut u8, fdt_size.into()) };
    // We do not need to validate the DT since it is already validated in pvmfw.
    let fdt = libfdt::Fdt::from_slice(fdt)?;

    #[allow(unused_mut)]
    let mut memory_range = fdt.first_memory_range()?;
    // "/memory" may include the pvmfw region, which we don't supported reusing in rialto, so
    // truncate it off if present.
    #[cfg(target_arch = "aarch64")]
    if memory_range.start == crosvm::PVMFW_START {
        memory_range.start = crosvm::MEM_START;
    }
    resize_available_memory(&memory_range).inspect_err(|_| {
        error!("Failed to use memory range value from DT: {memory_range:#x?}");
    })?;

    info!("main memory region: {memory_range:#?}");
    // TODO(ioffe): start vsock server to accept requests from the host.

    Ok(())
}

/// Entry point for this VM.
pub fn main(argv: &[usize]) {
    log::set_max_level(log::LevelFilter::Debug);
    // SAFETY: pvmfw passes a valid pointer to a valid `Fdt` to the guest kernel entry point.
    if let Err(e) = unsafe { try_main(argv[0]) } {
        error!("test vm failed: {e:?}");
        reboot()
    }
}

generate_image_header!();
main!(main);
configure_heap!(SIZE_128KB * 2);
