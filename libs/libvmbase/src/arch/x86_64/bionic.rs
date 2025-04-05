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

//! x86-64 Bionic libc support code

use core::arch::asm;

use crate::bionic::Tls;

/// Arbitrary data cache size to make libc happy
/// Based on bionic/libc/bionic/libc_init_common.cpp
#[no_mangle]
pub static __x86_data_cache_size: usize = 24 * 1024;

/// Arbitrary data cache size to make libc happy
/// Based on bionic/libc/bionic/libc_init_common.cpp
#[no_mangle]
pub static __x86_data_cache_size_half: usize = __x86_data_cache_size / 2;

/// Arbitrary size of cache to make libc happy
/// Based on bionic/libc/bionic/libc_init_common.cpp
#[no_mangle]
pub static __x86_shared_cache_size: usize = 4 * 1024 * 1024;

/// Arbitrary size of cache to make libc happy
/// Based on bionic/libc/bionic/libc_init_common.cpp
#[no_mangle]
pub static __x86_shared_cache_size_half: usize = __x86_shared_cache_size / 2;

/// Gets a pointer to the TLS from the FS register.
pub fn __get_tls() -> *mut Tls {
    let tls_ptr: *mut Tls;
    // SAFETY: TLS region and FS are initialized in entry.S.
    unsafe { asm!("mov {}, fs:[0]", out(reg) tls_ptr) };
    tls_ptr
}
