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

//! Wrappers around calls to the KVM hypervisor.

use super::{Hypervisor, MemSharingHypervisor};
use crate::{mem::SIZE_4KB, Error, Result};
use core::fmt::{self, Display, Formatter};

const KVM_HC_PKVM_OP: u32 = 20;
const PKVM_GHC_SHARE_MEM: u32 = KVM_HC_PKVM_OP + 1;
const PKVM_GHC_UNSHARE_MEM: u32 = KVM_HC_PKVM_OP + 2;

const KVM_ENOSYS: i64 = -1000;
const KVM_EINVAL: i64 = -22;

/// This CPUID returns the signature and can be used to determine if VM is running under pKVM, KVM
/// or not.
pub const KVM_CPUID_SIGNATURE: u32 = 0x40000000;

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
            KVM_ENOSYS => KvmError::NotSupported,
            KVM_EINVAL => KvmError::InvalidParameter,
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

pub(super) struct RegularKvmHypervisor;

impl RegularKvmHypervisor {
    pub(super) const CPUID: u128 = u128::from_le_bytes(*b"KVMKVMKVM\0\0\0\0\0\0\0");
}

impl Hypervisor for RegularKvmHypervisor {}

pub(super) struct ProtectedKvmHypervisor;

impl ProtectedKvmHypervisor {
    pub(super) const CPUID: u128 = u128::from_le_bytes(*b"PKVMPKVMPKVM\0\0\0\0");
}

impl Hypervisor for ProtectedKvmHypervisor {
    fn as_mem_sharer(&self) -> Option<&dyn MemSharingHypervisor> {
        Some(self)
    }
}

macro_rules! vmcall {
    ($hypcall:expr, $base:expr, $size:expr) => {{
        let ret;
        // SAFETY:
        // Any undeclared register aren't clobbered except rbx but rbx value is restored at the end
        // of the asm block.
        unsafe {
            core::arch::asm!(
                "xchg %rbx, {0:r}",
                "vmcall",
                "xchg %rbx, {0:r}",
                in(reg) $base,
                inout("rax") $hypcall => ret,
                in("rcx") $size,
                options(att_syntax, nomem));
        };
        ret
    }};
}

macro_rules! cpuid {
    ($hypcall:expr) => {{
        let ret_1: u32;
        let ret_2: u32;
        let ret_3: u32;
        // SAFETY:
        // Any undeclared register aren't clobbered except rbx but rbx value is restored at the end
        // of the asm block.
        unsafe {
            // The argument for cpuid is passed via rax and in case of KVM_CPUID_SIGNATURE returned
            // via rbx, rcx and rdx. Ideally using named arguments in inline asm for rbx would be
            // much more straightforward but when rbx is directly used LLVM complains that: error:
            // cannot use register `bx`: rbx is used internally by LLVM and cannot be used as an
            // operand for inline asm
            //
            // Therefore use temp register to store rbx content and restore it back after cpuid
            // call.
            core::arch::asm!(
                "xchg %rbx, {0:r}",
                "cpuid",
                "xchg %rbx, {0:r}",
                out(reg) ret_1, in("eax") $hypcall, out("rcx") ret_2, out ("rdx") ret_3,
                options(att_syntax, nomem));
        };
        ((ret_3 as u128) << 64) | ((ret_2 as u128) << 32) | (ret_1 as u128)
    }};
}

impl MemSharingHypervisor for ProtectedKvmHypervisor {
    fn share(&self, base_ipa: u64) -> Result<()> {
        let ret: u32 = vmcall!(PKVM_GHC_SHARE_MEM, base_ipa, SIZE_4KB);

        if ret != 0 {
            return Err(Error::KvmError(KvmError::from(ret as i32), PKVM_GHC_SHARE_MEM));
        }

        Ok(())
    }

    fn unshare(&self, base_ipa: u64) -> Result<()> {
        let ret: u32 = vmcall!(PKVM_GHC_UNSHARE_MEM, base_ipa, SIZE_4KB);
        if ret != 0 {
            return Err(Error::KvmError(KvmError::from(ret as i32), PKVM_GHC_UNSHARE_MEM));
        }

        Ok(())
    }

    fn granule(&self) -> Result<usize> {
        Ok(SIZE_4KB)
    }
}

use crate::hypervisor::HypervisorBackend;

pub(crate) fn determine_hyp_type() -> Result<HypervisorBackend> {
    let cpuid: u128 = cpuid!(KVM_CPUID_SIGNATURE);

    match cpuid {
        RegularKvmHypervisor::CPUID => Ok(HypervisorBackend::RegularKvm),
        ProtectedKvmHypervisor::CPUID => Ok(HypervisorBackend::ProtectedKvm),
        c => Err(Error::UnsupportedHypervisor(c)),
    }
}
