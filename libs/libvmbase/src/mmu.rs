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

use core::fmt;
use core::ops::Range;

/// Enum describing memory mapping errors
#[derive(Clone, Debug, Eq, PartialEq)]
// TODO(ptosi): Make this pub(crate) once PageTables are properly encapsulated.
pub enum MmuError {
    /// The address is invalid.
    BadAddress(usize),
    /// The region is invalid.
    BadRegion(Range<usize>),
    /// The flags are invalid.
    BadFlags(usize),
    /// An update operation failed.
    UpdateFailed { addr: usize, flags: Option<usize> },
    /// The region can't be updated.
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
