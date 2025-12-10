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

/// Errors for MemoryTracker operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum MemoryTrackerError {
    /// MemoryTracker not configured or deactivated.
    #[error("MemoryTracker is not available")]
    Unavailable,
    /// Tried to modify the memory base address.
    #[error("Received different base address")]
    DifferentBaseAddress,
    /// Tried to shrink to a larger memory size.
    #[error("Tried to shrink to a larger memory size")]
    SizeTooLarge,
    /// Tracked regions would not fit in memory size.
    #[error("Tracked regions would not fit in memory size")]
    SizeTooSmall,
    /// Reached limit number of tracked regions.
    #[error("Reached limit number of tracked regions")]
    Full,
    /// Region is out of the tracked memory address space.
    #[error("Region is out of the tracked memory address space")]
    OutOfRange,
    /// New region overlaps with tracked regions.
    #[error("New region overlaps with tracked regions")]
    Overlaps,
    /// Region is not present in the tracked regions.
    #[error("Region is not mapped")]
    NotMapped,
    /// Region couldn't be mapped.
    #[error("Failed to map the new region")]
    FailedToMap,
    /// Region couldn't be unmapped.
    #[error("Failed to unmap the new region")]
    FailedToUnmap,
    /// Error from the interaction with the hypervisor.
    #[error("{0}")]
    Hypervisor(#[from] hypervisor_backends::Error),
    /// Failure to set `SHARED_MEMORY`.
    #[error("Failed to set SHARED_MEMORY")]
    SharedMemorySetFailure,
    /// Failure to set `SHARED_POOL`.
    #[error("Failed to set SHARED_POOL")]
    SharedPoolSetFailure,
    /// Rejected request to map footer that is already mapped.
    #[error("Refused to map image footer again")]
    FooterAlreadyMapped,
    /// Invalid page table entry.
    #[error("Page table entry is not valid")]
    InvalidPte,
    /// Failed to set PTE dirty state.
    #[error("Failed to set PTE dirty state")]
    SetPteDirtyFailed,
    /// Attempting to MMIO_GUARD_MAP more than once the same region.
    #[error("Attempted to share the same MMIO region at {0:#x} twice")]
    DuplicateMmioShare(usize),
    /// The MMIO_GUARD granule used by the hypervisor is not supported.
    #[error("Unsupported MMIO guard granule: {0}")]
    UnsupportedMmioGuardGranule(usize),
}
