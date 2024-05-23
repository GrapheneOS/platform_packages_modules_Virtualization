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

//! Error relating to memory management.

use core::fmt;

use crate::hyp;

/// Errors for MemoryTracker operations.
#[derive(Debug, Clone)]
pub enum MemoryTrackerError {
    /// Tried to modify the memory base address.
    DifferentBaseAddress,
    /// Tried to shrink to a larger memory size.
    SizeTooLarge,
    /// Tracked regions would not fit in memory size.
    SizeTooSmall,
    /// Reached limit number of tracked regions.
    Full,
    /// Region is out of the tracked memory address space.
    OutOfRange,
    /// New region overlaps with tracked regions.
    Overlaps,
    /// Region couldn't be mapped.
    FailedToMap,
    /// Region couldn't be unmapped.
    FailedToUnmap,
    /// Error from the interaction with the hypervisor.
    Hypervisor(hyp::Error),
    /// Failure to set `SHARED_MEMORY`.
    SharedMemorySetFailure,
    /// Failure to set `SHARED_POOL`.
    SharedPoolSetFailure,
    /// Invalid page table entry.
    InvalidPte,
    /// Failed to flush memory region.
    FlushRegionFailed,
    /// Failed to set PTE dirty state.
    SetPteDirtyFailed,
    /// Attempting to MMIO_GUARD_MAP more than once the same region.
    DuplicateMmioShare(usize),
    /// The MMIO_GUARD granule used by the hypervisor is not supported.
    UnsupportedMmioGuardGranule(usize),
}

impl fmt::Display for MemoryTrackerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::DifferentBaseAddress => write!(f, "Received different base address"),
            Self::SizeTooLarge => write!(f, "Tried to shrink to a larger memory size"),
            Self::SizeTooSmall => write!(f, "Tracked regions would not fit in memory size"),
            Self::Full => write!(f, "Reached limit number of tracked regions"),
            Self::OutOfRange => write!(f, "Region is out of the tracked memory address space"),
            Self::Overlaps => write!(f, "New region overlaps with tracked regions"),
            Self::FailedToMap => write!(f, "Failed to map the new region"),
            Self::FailedToUnmap => write!(f, "Failed to unmap the new region"),
            Self::Hypervisor(e) => e.fmt(f),
            Self::SharedMemorySetFailure => write!(f, "Failed to set SHARED_MEMORY"),
            Self::SharedPoolSetFailure => write!(f, "Failed to set SHARED_POOL"),
            Self::InvalidPte => write!(f, "Page table entry is not valid"),
            Self::FlushRegionFailed => write!(f, "Failed to flush memory region"),
            Self::SetPteDirtyFailed => write!(f, "Failed to set PTE dirty state"),
            Self::DuplicateMmioShare(addr) => {
                write!(f, "Attempted to share the same MMIO region at {addr:#x} twice")
            }
            Self::UnsupportedMmioGuardGranule(g) => {
                write!(f, "Unsupported MMIO guard granule: {g}")
            }
        }
    }
}

impl From<hyp::Error> for MemoryTrackerError {
    fn from(e: hyp::Error) -> Self {
        Self::Hypervisor(e)
    }
}
