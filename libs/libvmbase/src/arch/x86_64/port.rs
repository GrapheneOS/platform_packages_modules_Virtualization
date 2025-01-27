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

//! x86-64 port I/O operations

use core::arch::asm;

/// Write byte to I/O bus at defined address
///
/// # Safety
///
/// The caller must ensure that writing to this port will not cause any memory-safety side
/// effects.
#[inline]
pub unsafe fn write_u8(port: u16, value: u8) {
    // SAFETY: Caller is responsible for ensuring safety of accessing this I/O port.
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}
