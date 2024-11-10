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

//! Low-level CPU-specific operations.

#[cfg(target_arch = "aarch64")]
pub mod aarch64;

/// Write with well-defined compiled behavior.
///
/// See https://github.com/rust-lang/rust/issues/131894
///
/// # Safety
///
/// `dst` must be valid for writes.
#[inline]
pub unsafe fn write_volatile_u8(dst: *mut u8, src: u8) {
    cfg_if::cfg_if! {
        if #[cfg(target_arch = "aarch64")] {
            // SAFETY: `dst` is valid for writes.
            unsafe { aarch64::strb(dst, src) }
        } else {
            compile_error!("Unsupported target_arch")
        }
    }
}

/// Flush `size` bytes of data cache by virtual address.
#[inline]
pub(crate) fn flush_region(start: usize, size: usize) {
    cfg_if::cfg_if! {
        if #[cfg(target_arch = "aarch64")] {
            let line_size = aarch64::min_dcache_line_size();
            let end = start + size;
            let start = crate::util::unchecked_align_down(start, line_size);

            for line in (start..end).step_by(line_size) {
                crate::dc!("cvau", line);
            }
        } else {
            compile_error!("Unsupported target_arch")
        }
    }
}
