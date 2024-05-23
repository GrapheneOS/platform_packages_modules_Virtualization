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

//! Utility functions for memory management.

use crate::read_sysreg;
use crate::util::unchecked_align_down;
use core::arch::asm;
use core::ptr::NonNull;
use zeroize::Zeroize;

/// The size of a 4KB memory in bytes.
pub const SIZE_4KB: usize = 4 << 10;
/// The size of a 16KB memory in bytes.
pub const SIZE_16KB: usize = 16 << 10;
/// The size of a 64KB memory in bytes.
pub const SIZE_64KB: usize = 64 << 10;
/// The size of a 128KB memory in bytes.
pub const SIZE_128KB: usize = 128 << 10;
/// The size of a 2MB memory in bytes.
pub const SIZE_2MB: usize = 2 << 20;
/// The size of a 4MB memory in bytes.
pub const SIZE_4MB: usize = 4 << 20;

/// The page size in bytes assumed by vmbase - 4 KiB.
pub const PAGE_SIZE: usize = SIZE_4KB;

/// Reads the number of words in the smallest cache line of all the data caches and unified caches.
#[inline]
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
pub(super) fn flush_region(start: usize, size: usize) {
    let line_size = min_dcache_line_size();
    let end = start + size;
    let start = unchecked_align_down(start, line_size);

    for line in (start..end).step_by(line_size) {
        // SAFETY: Clearing cache lines shouldn't have Rust-visible side effects.
        unsafe {
            asm!(
                "dc cvau, {x}",
                x = in(reg) line,
                options(nomem, nostack, preserves_flags),
            )
        }
    }
}

/// Flushes the slice to the point of unification.
#[inline]
pub fn flush(reg: &[u8]) {
    flush_region(reg.as_ptr() as usize, reg.len())
}

/// Overwrites the slice with zeroes, to the point of unification.
#[inline]
pub fn flushed_zeroize(reg: &mut [u8]) {
    reg.zeroize();
    flush(reg)
}

/// Computes the address of the 4KiB page containing a given address.
pub const fn page_4kb_of(addr: usize) -> usize {
    unchecked_align_down(addr, SIZE_4KB)
}

/// Returns the intermediate physical address corresponding to the given virtual address.
///
/// As we use identity mapping for everything, this is just a cast, but it's useful to use it to be
/// explicit about where we are converting from virtual to physical address.
pub(crate) fn virt_to_phys(vaddr: NonNull<u8>) -> usize {
    vaddr.as_ptr() as _
}

/// Returns a pointer for the virtual address corresponding to the given non-zero intermediate
/// physical address.
///
/// Panics if `paddr` is 0.
pub(crate) fn phys_to_virt(paddr: usize) -> NonNull<u8> {
    NonNull::new(paddr as _).unwrap()
}
