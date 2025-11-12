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

//! Architecture-agnostic interface to the MMU and page tables.

use crate::layout;
use core::fmt;
use core::ops::Range;
use core::result;

pub(crate) type MmuResult<T> = result::Result<T, MmuError>;

/// Enum describing memory mapping errors
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MmuError {
    /// The address is invalid.
    BadAddress(usize),
    /// The region is invalid.
    BadRegion(Range<usize>),
    /// The flags are invalid.
    BadFlags(usize),
    /// An update operation failed.
    UpdateFailed { addr: usize, flags: Option<usize> },
    /// The region can't be updated.
    #[cfg(not(target_arch = "x86_64"))]
    RegionLocked(Range<usize>),
}

impl fmt::Display for MmuError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::BadAddress(a) => write!(f, "Invalid address {a:#x}"),
            Self::BadRegion(r) => write!(f, "Invalid region {r:#x?}"),
            Self::BadFlags(g) => write!(f, "Invalid flags {g:#x}"),
            Self::UpdateFailed { addr, flags } => {
                write!(f, "Update operation failed at {addr:#x} (flags={flags:#x?})")
            }
            #[cfg(not(target_arch = "x86_64"))]
            Self::RegionLocked(r) => write!(f, "Region {r:#x?} is locked"),
        }
    }
}

#[cfg(target_arch = "aarch64")]
impl From<aarch64_paging::MapError> for MmuError {
    fn from(e: aarch64_paging::MapError) -> Self {
        use aarch64_paging::MapError;
        match e {
            MapError::InvalidVirtualAddress(a) => Self::BadAddress(a.0),
            MapError::AddressRange(a) => Self::BadAddress(a.0),
            MapError::RegionBackwards(r) => Self::BadRegion(r.start().0..r.end().0),
            MapError::PteUpdateFault(d) => Self::UpdateFailed {
                addr: 0, // TODO(libsmccc updated): d.output_address().0,
                flags: d.flags().map(|f| f.bits()),
            },
            MapError::InvalidFlags(f) => Self::BadFlags(f.bits()),
            MapError::BreakBeforeMakeViolation(r) => Self::RegionLocked(r.start().0..r.end().0),
        }
    }
}

pub(crate) trait MmuOps: Default {
    /// Produces an instance that can safely replace the static PTs on `.activate()`.
    fn clone_static_page_tables() -> Self {
        let mut mmu = Self::default();

        if let Some(console_uart_page) = layout::console_uart_page() {
            mmu.map_device(&console_uart_page).unwrap();
        }
        mmu.map_code(&layout::text_range()).unwrap();
        mmu.map_rodata(&layout::rodata_range()).unwrap();
        mmu.map_data(&layout::data_bss_range()).unwrap();
        mmu.map_data(&layout::eh_stack_range()).unwrap();
        mmu.map_data(&layout::stack_range()).unwrap();

        mmu
    }

    /// Switches to the dynamic page tables.
    ///
    /// Until this function is called, calls to other functions modify PTs that aren't live.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the instance has valid and identical mappings for the code
    /// being currently executed. Otherwise, the Rust execution model (on which the borrow checker
    /// relies) would be violated.
    unsafe fn activate(&mut self);

    /// Maps `range` as "code" memory (executable, read-only, cached).
    fn map_code(&mut self, range: &Range<usize>) -> MmuResult<()>;

    /// Maps `range` as "read-only data" memory (non-executable, read-only, cached).
    fn map_rodata(&mut self, range: &Range<usize>) -> MmuResult<()>;

    /// Maps `range` as "device" memory (non-executable, R/W, non-cached).
    fn map_device(&mut self, range: &Range<usize>) -> MmuResult<()>;

    /// Maps `range` as "device" memory if previously marked lazy, otherwise fails.
    fn map_device_expect_lazy(&mut self, range: &Range<usize>) -> MmuResult<()>;

    /// Marks `range` for map_device_expect_lazy(), does NOT map the memory.
    fn mark_as_lazy_device(&mut self, range: &Range<usize>) -> MmuResult<()>;

    /// Maps `range` as "data" memory (non-executable, R/W, cached).
    fn map_data(&mut self, range: &Range<usize>) -> MmuResult<()>;

    /// Maps `range` as "data" memory (non-executable, R/W, cached) with dirty state tracking.
    ///
    /// This might use a CPU hardware feature to automatically mark the pages dirty on stores or
    /// prepares the range for software tracking, through `mark_data_dirty()`. Compared to
    /// `map_data()`, this may result in more memory being used and/or less optimal mappings.
    fn map_data_track_dirty_state(&mut self, range: &Range<usize>) -> MmuResult<()>;

    /// Marks the "data" `range` as dirty if properly mapped for tracking, otherwise fails.
    fn mark_data_dirty(&mut self, range: &Range<usize>) -> MmuResult<()>;

    /// Unmaps the given range of memory.
    fn unmap(&mut self, range: &Range<usize>) -> MmuResult<()>;

    /// Acts as a barrier, ensuring that the dirty state is properly updated on return.
    fn sync_dirty_state(&mut self) -> MmuResult<()>;

    /// Flushes the data caches over every page of `range` that might have been dirtied.
    ///
    /// The `range` must have been previously mapped with `map_data_track_dirty_state()` and/or
    /// `map_data()`. Pages mapped with the former will always be flushed and with the latter might
    /// only be flushed if actually dirtied.
    fn flush_dirty_pages(&mut self, range: &Range<usize>) -> MmuResult<()>;
}
