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

//! This module regroups some common traits shared by all the hypervisors.

use crate::hyp::Result;

/// Trait for the hypervisor.
pub trait Hypervisor {
    /// Returns the hypervisor's MMIO_GUARD implementation, if any.
    fn as_mmio_guard(&self) -> Option<&dyn MmioGuardedHypervisor> {
        None
    }

    /// Returns the hypervisor's dynamic memory sharing implementation, if any.
    fn as_mem_sharer(&self) -> Option<&dyn MemSharingHypervisor> {
        None
    }

    /// Returns the hypervisor's device assigning implementation, if any.
    fn as_device_assigner(&self) -> Option<&dyn DeviceAssigningHypervisor> {
        None
    }
}

pub trait MmioGuardedHypervisor {
    /// Enrolls with the MMIO guard so that all MMIO will be blocked unless allow-listed with
    /// `MmioGuardedHypervisor::map`.
    fn enroll(&self) -> Result<()>;

    /// Maps a page containing the given memory address to the hypervisor MMIO guard.
    /// The page size corresponds to the MMIO guard granule size.
    fn map(&self, addr: usize) -> Result<()>;

    /// Unmaps a page containing the given memory address from the hypervisor MMIO guard.
    /// The page size corresponds to the MMIO guard granule size.
    fn unmap(&self, addr: usize) -> Result<()>;

    /// Returns the MMIO guard granule size in bytes.
    fn granule(&self) -> Result<usize>;
}

pub trait MemSharingHypervisor {
    /// Shares a region of memory with host, granting it read, write and execute permissions.
    /// The size of the region is equal to the memory protection granule returned by
    /// [`hyp_meminfo`].
    fn share(&self, base_ipa: u64) -> Result<()>;

    /// Revokes access permission from host to a memory region previously shared with
    /// [`mem_share`]. The size of the region is equal to the memory protection granule returned by
    /// [`hyp_meminfo`].
    fn unshare(&self, base_ipa: u64) -> Result<()>;

    /// Returns the memory protection granule size in bytes.
    fn granule(&self) -> Result<usize>;
}

/// Device assigning hypervisor
pub trait DeviceAssigningHypervisor {
    /// Returns MMIO token.
    fn get_phys_mmio_token(&self, base_ipa: u64, size: u64) -> Result<u64>;

    /// Returns DMA token as a tuple of (phys_iommu_id, phys_sid).
    fn get_phys_iommu_token(&self, pviommu_id: u64, vsid: u64) -> Result<(u64, u64)>;
}
