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

//! Miscellaneous helper functions.

use core::arch::asm;
use core::ops::Range;
use zeroize::Zeroize;

pub const SIZE_4KB: usize = 4 << 10;
pub const SIZE_2MB: usize = 2 << 20;
pub const SIZE_4MB: usize = 4 << 20;

pub const GUEST_PAGE_SIZE: usize = SIZE_4KB;
pub const PVMFW_PAGE_SIZE: usize = SIZE_4KB;

/// Read a value from a system register.
#[macro_export]
macro_rules! read_sysreg {
    ($sysreg:literal) => {{
        let mut r: usize;
        // Safe because it reads a system register and does not affect Rust.
        unsafe {
            core::arch::asm!(
                concat!("mrs {}, ", $sysreg),
                out(reg) r,
                options(nomem, nostack, preserves_flags),
            )
        }
        r
    }};
}

/// Write a value to a system register.
///
/// # Safety
///
/// Callers must ensure that side effects of updating the system register are properly handled.
#[macro_export]
macro_rules! write_sysreg {
    ($sysreg:literal, $val:expr) => {{
        let value: usize = $val;
        core::arch::asm!(
            concat!("msr ", $sysreg, ", {}"),
            in(reg) value,
            options(nomem, nostack, preserves_flags),
        )
    }};
}

/// Computes the largest multiple of the provided alignment smaller or equal to the address.
///
/// Note: the result is undefined if alignment isn't a power of two.
pub const fn unchecked_align_down(addr: usize, alignment: usize) -> usize {
    addr & !(alignment - 1)
}

/// Computes the smallest multiple of the provided alignment larger or equal to the address.
///
/// Note: the result is undefined if alignment isn't a power of two and may wrap to 0.
pub const fn unchecked_align_up(addr: usize, alignment: usize) -> usize {
    unchecked_align_down(addr + alignment - 1, alignment)
}

/// Safe wrapper around unchecked_align_up() that validates its assumptions and doesn't wrap.
pub const fn align_up(addr: usize, alignment: usize) -> Option<usize> {
    if !alignment.is_power_of_two() {
        None
    } else if let Some(s) = addr.checked_add(alignment - 1) {
        Some(unchecked_align_down(s, alignment))
    } else {
        None
    }
}

/// Performs an integer division rounding up.
///
/// Note: Returns None if den isn't a power of two.
pub const fn ceiling_div(num: usize, den: usize) -> Option<usize> {
    let Some(r) = align_up(num, den) else {
        return None;
    };

    r.checked_div(den)
}

/// Aligns the given address to the given alignment, if it is a power of two.
///
/// Returns `None` if the alignment isn't a power of two.
#[allow(dead_code)] // Currently unused but might be needed again.
pub const fn align_down(addr: usize, alignment: usize) -> Option<usize> {
    if !alignment.is_power_of_two() {
        None
    } else {
        Some(unchecked_align_down(addr, alignment))
    }
}

/// Computes the address of the 4KiB page containing a given address.
pub const fn page_4kb_of(addr: usize) -> usize {
    unchecked_align_down(addr, SIZE_4KB)
}

#[inline]
/// Read the number of words in the smallest cache line of all the data caches and unified caches.
pub fn min_dcache_line_size() -> usize {
    const DMINLINE_SHIFT: usize = 16;
    const DMINLINE_MASK: usize = 0xf;
    let ctr_el0 = read_sysreg!("ctr_el0");

    // DminLine: log2 of the number of words in the smallest cache line of all the data caches.
    let dminline = (ctr_el0 >> DMINLINE_SHIFT) & DMINLINE_MASK;

    1 << dminline
}

/// Flush `size` bytes of data cache by virtual address.
#[inline]
pub fn flush_region(start: usize, size: usize) {
    let line_size = min_dcache_line_size();
    let end = start + size;
    let start = unchecked_align_down(start, line_size);

    for line in (start..end).step_by(line_size) {
        // SAFETY - Clearing cache lines shouldn't have Rust-visible side effects.
        unsafe {
            asm!(
                "dc cvac, {x}",
                x = in(reg) line,
                options(nomem, nostack, preserves_flags),
            )
        }
    }
}

#[inline]
/// Flushes the slice to the point of unification.
pub fn flush(reg: &[u8]) {
    flush_region(reg.as_ptr() as usize, reg.len())
}

#[inline]
/// Overwrites the slice with zeroes, to the point of unification.
pub fn flushed_zeroize(reg: &mut [u8]) {
    reg.zeroize();
    flush(reg)
}

/// Flatten [[T; N]] into &[T]
/// TODO: use slice::flatten when it graduates from experimental
pub fn flatten<T, const N: usize>(original: &[[T; N]]) -> &[T] {
    // SAFETY: no overflow because original (whose size is len()*N) is already in memory
    let len = original.len() * N;
    // SAFETY: [T] has the same layout as [T;N]
    unsafe { core::slice::from_raw_parts(original.as_ptr().cast(), len) }
}

/// Trait to check containment of one range within another.
pub(crate) trait RangeExt {
    /// Returns true if `self` is contained within the `other` range.
    fn is_within(&self, other: &Self) -> bool;
}

impl<T: PartialOrd> RangeExt for Range<T> {
    fn is_within(&self, other: &Self) -> bool {
        self.start >= other.start && self.end <= other.end
    }
}

/// Create &CStr out of &str literal
#[macro_export]
macro_rules! cstr {
    ($str:literal) => {{
        CStr::from_bytes_with_nul(concat!($str, "\0").as_bytes()).unwrap()
    }};
}
