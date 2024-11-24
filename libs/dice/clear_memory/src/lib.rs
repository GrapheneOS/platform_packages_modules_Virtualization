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

//! Routine for clearing memory containing confidential data, used by the open-dice library.
//!
//! Clients should link against this library along the libopen_dice_*_baremetal libraries.

#![no_std]

use core::ffi::c_void;
use core::slice;
use vmbase::memory::flushed_zeroize;

/// Zeroes data over the provided address range & flushes data caches.
///
/// # Safety
///
/// The provided address and size must be to an address range that is valid for read and write
/// from a single allocation (e.g. stack array).
#[no_mangle]
unsafe extern "C" fn DiceClearMemory(_ctx: *mut c_void, size: usize, addr: *mut c_void) {
    // SAFETY: We require our caller to provide a valid range within a single object. The
    // open-dice always calls this on individual stack-allocated arrays which ensures that.
    let region = unsafe { slice::from_raw_parts_mut(addr as *mut u8, size) };
    flushed_zeroize(region);
}
