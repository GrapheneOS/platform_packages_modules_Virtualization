// Copyright 2023, The Android Open Source Project
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

//! Utility functions.

use aarch64_paging::paging::MemoryRegion;
use core::ops::Range;

/// Flatten [[T; N]] into &[T]
/// TODO: use slice::flatten when it graduates from experimental
pub fn flatten<T, const N: usize>(original: &[[T; N]]) -> &[T] {
    // SAFETY: no overflow because original (whose size is len()*N) is already in memory
    let len = original.len() * N;
    // SAFETY: [T] has the same layout as [T;N]
    unsafe { core::slice::from_raw_parts(original.as_ptr().cast(), len) }
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

/// Aligns the given address to the given alignment, if it is a power of two.
///
/// Returns `None` if the alignment isn't a power of two.
#[allow(dead_code)] // Currently unused but might be needed again.
const fn align_down(addr: usize, alignment: usize) -> Option<usize> {
    if !alignment.is_power_of_two() {
        None
    } else {
        Some(unchecked_align_down(addr, alignment))
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

/// Trait to check containment of one range within another.
pub trait RangeExt {
    /// Returns true if `self` is contained within the `other` range.
    fn is_within(&self, other: &Self) -> bool;

    /// Returns true if `self` overlaps with the `other` range.
    fn overlaps(&self, other: &Self) -> bool;
}

impl<T: PartialOrd> RangeExt for Range<T> {
    fn is_within(&self, other: &Self) -> bool {
        self.start >= other.start && self.end <= other.end
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

impl RangeExt for MemoryRegion {
    fn is_within(&self, other: &Self) -> bool {
        self.start() >= other.start() && self.end() <= other.end()
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.start() < other.end() && other.start() < self.end()
    }
}
