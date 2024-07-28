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

//! Wrappers around calls to the KVM hypervisor.

use core::fmt::{self, Display, Formatter};

use super::{DeviceAssigningHypervisor, Hypervisor, MemSharingHypervisor, MmioGuardedHypervisor};
use crate::{
    hyp::{Error, Result},
    memory::page_4kb_of,
};

use smccc::{
    error::{positive_or_error_64, success_or_error_32, success_or_error_64},
    hvc64,
};
use uuid::{uuid, Uuid};

/// Error from a KVM HVC call.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum KvmError {
    /// The call is not supported by the implementation.
    NotSupported,
    /// One of the call parameters has a non-supported value.
    InvalidParameter,
    /// There was an unexpected return value.
    Unknown(i64),
}

impl From<i64> for KvmError {
    fn from(value: i64) -> Self {
        match value {
            -1 => KvmError::NotSupported,
            -3 => KvmError::InvalidParameter,
            _ => KvmError::Unknown(value),
        }
    }
}

impl From<i32> for KvmError {
    fn from(value: i32) -> Self {
        i64::from(value).into()
    }
}

impl Display for KvmError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::NotSupported => write!(f, "KVM call not supported"),
            Self::InvalidParameter => write!(f, "KVM call received non-supported value"),
            Self::Unknown(e) => write!(f, "Unknown return value from KVM {} ({0:#x})", e),
        }
    }
}

const ARM_SMCCC_KVM_FUNC_HYP_MEMINFO: u32 = 0xc6000002;
const ARM_SMCCC_KVM_FUNC_MEM_SHARE: u32 = 0xc6000003;
const ARM_SMCCC_KVM_FUNC_MEM_UNSHARE: u32 = 0xc6000004;

const VENDOR_HYP_KVM_MMIO_GUARD_INFO_FUNC_ID: u32 = 0xc6000005;
const VENDOR_HYP_KVM_MMIO_GUARD_ENROLL_FUNC_ID: u32 = 0xc6000006;
const VENDOR_HYP_KVM_MMIO_GUARD_MAP_FUNC_ID: u32 = 0xc6000007;
const VENDOR_HYP_KVM_MMIO_GUARD_UNMAP_FUNC_ID: u32 = 0xc6000008;

const VENDOR_HYP_KVM_DEV_REQ_MMIO_FUNC_ID: u32 = 0xc6000012;
const VENDOR_HYP_KVM_DEV_REQ_DMA_FUNC_ID: u32 = 0xc600001b;

pub(super) struct RegularKvmHypervisor;

impl RegularKvmHypervisor {
    // Based on ARM_SMCCC_VENDOR_HYP_UID_KVM_REG values listed in Linux kernel source:
    // https://github.com/torvalds/linux/blob/master/include/linux/arm-smccc.h
    pub(super) const UUID: Uuid = uuid!("28b46fb6-2ec5-11e9-a9ca-4b564d003a74");
}

impl Hypervisor for RegularKvmHypervisor {}

pub(super) struct ProtectedKvmHypervisor;

impl Hypervisor for ProtectedKvmHypervisor {
    fn as_mmio_guard(&self) -> Option<&dyn MmioGuardedHypervisor> {
        Some(self)
    }

    fn as_mem_sharer(&self) -> Option<&dyn MemSharingHypervisor> {
        Some(self)
    }

    fn as_device_assigner(&self) -> Option<&dyn DeviceAssigningHypervisor> {
        Some(self)
    }
}

impl MmioGuardedHypervisor for ProtectedKvmHypervisor {
    fn enroll(&self) -> Result<()> {
        let args = [0u64; 17];
        match success_or_error_64(hvc64(VENDOR_HYP_KVM_MMIO_GUARD_ENROLL_FUNC_ID, args)[0]) {
            Ok(()) => Ok(()),
            Err(KvmError::NotSupported) => Err(Error::MmioGuardNotSupported),
            Err(e) => Err(Error::KvmError(e, VENDOR_HYP_KVM_MMIO_GUARD_ENROLL_FUNC_ID)),
        }
    }

    fn map(&self, addr: usize) -> Result<()> {
        let mut args = [0u64; 17];
        args[0] = page_4kb_of(addr).try_into().unwrap();

        if cfg!(feature = "compat_android_13") {
            let res = hvc64(VENDOR_HYP_KVM_MMIO_GUARD_MAP_FUNC_ID, args)[0];
            // pKVM returns i32 instead of the intended i64 in Android 13.
            return success_or_error_32(res as u32)
                .map_err(|e| Error::KvmError(e, VENDOR_HYP_KVM_MMIO_GUARD_MAP_FUNC_ID));
        }

        checked_hvc64_expect_zero(VENDOR_HYP_KVM_MMIO_GUARD_MAP_FUNC_ID, args)
    }

    fn unmap(&self, addr: usize) -> Result<()> {
        let mut args = [0u64; 17];
        args[0] = page_4kb_of(addr).try_into().unwrap();

        if cfg!(feature = "compat_android_13") {
            let res = hvc64(VENDOR_HYP_KVM_MMIO_GUARD_UNMAP_FUNC_ID, args)[0];
            // pKVM returns NOT_SUPPORTED for SUCCESS in Android 13.
            return match success_or_error_64(res) {
                Err(KvmError::NotSupported) | Ok(_) => Ok(()),
                Err(e) => Err(Error::KvmError(e, VENDOR_HYP_KVM_MMIO_GUARD_UNMAP_FUNC_ID)),
            };
        }

        checked_hvc64_expect_zero(VENDOR_HYP_KVM_MMIO_GUARD_UNMAP_FUNC_ID, args)
    }

    fn granule(&self) -> Result<usize> {
        let args = [0u64; 17];
        let granule = checked_hvc64(VENDOR_HYP_KVM_MMIO_GUARD_INFO_FUNC_ID, args)?;
        Ok(granule.try_into().unwrap())
    }
}

impl MemSharingHypervisor for ProtectedKvmHypervisor {
    fn share(&self, base_ipa: u64) -> Result<()> {
        let mut args = [0u64; 17];
        args[0] = base_ipa;

        checked_hvc64_expect_zero(ARM_SMCCC_KVM_FUNC_MEM_SHARE, args)
    }

    fn unshare(&self, base_ipa: u64) -> Result<()> {
        let mut args = [0u64; 17];
        args[0] = base_ipa;

        checked_hvc64_expect_zero(ARM_SMCCC_KVM_FUNC_MEM_UNSHARE, args)
    }

    fn granule(&self) -> Result<usize> {
        let args = [0u64; 17];
        let granule = checked_hvc64(ARM_SMCCC_KVM_FUNC_HYP_MEMINFO, args)?;
        Ok(granule.try_into().unwrap())
    }
}

impl DeviceAssigningHypervisor for ProtectedKvmHypervisor {
    fn get_phys_mmio_token(&self, base_ipa: u64, size: u64) -> Result<u64> {
        let mut args = [0u64; 17];
        args[0] = base_ipa;
        args[1] = size;

        let ret = checked_hvc64_expect_results(VENDOR_HYP_KVM_DEV_REQ_MMIO_FUNC_ID, args)?;
        Ok(ret[0])
    }

    fn get_phys_iommu_token(&self, pviommu_id: u64, vsid: u64) -> Result<(u64, u64)> {
        let mut args = [0u64; 17];
        args[0] = pviommu_id;
        args[1] = vsid;

        let ret = checked_hvc64_expect_results(VENDOR_HYP_KVM_DEV_REQ_DMA_FUNC_ID, args)?;
        Ok((ret[0], ret[1]))
    }
}

fn checked_hvc64_expect_zero(function: u32, args: [u64; 17]) -> Result<()> {
    success_or_error_64(hvc64(function, args)[0]).map_err(|e| Error::KvmError(e, function))
}

fn checked_hvc64(function: u32, args: [u64; 17]) -> Result<u64> {
    positive_or_error_64(hvc64(function, args)[0]).map_err(|e| Error::KvmError(e, function))
}

fn checked_hvc64_expect_results(function: u32, args: [u64; 17]) -> Result<[u64; 17]> {
    let [ret, results @ ..] = hvc64(function, args);
    success_or_error_64(ret).map_err(|e| Error::KvmError(e, function))?;
    Ok(results)
}
