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

//! Basic functionality for bare-metal binaries to run in a VM under crosvm.

#![no_std]

extern crate alloc;

pub mod arch;
pub mod bionic;
pub mod console;
mod entry;
pub mod exceptions;
pub mod fdt;
pub mod heap;
mod hvc;
pub mod hyp;
pub mod layout;
pub mod linker;
pub mod logger;
pub mod memory;
pub mod power;
pub mod rand;
pub mod uart;
pub mod util;
pub mod virtio;

use core::panic::PanicInfo;
use power::reboot;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    eprintln!("{}", info);
    reboot()
}
