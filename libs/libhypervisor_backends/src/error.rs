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

//! Error and Result types for hypervisor.

use core::{fmt, result};

#[cfg(target_arch = "aarch64")]
use super::hypervisor::GeniezoneError;
use super::hypervisor::KvmError;
#[cfg(target_arch = "aarch64")]
use uuid::Uuid;

/// Result type with hypervisor error.
pub type Result<T> = result::Result<T, Error>;

/// Hypervisor error.
#[derive(Debug, Clone)]
pub enum Error {
    /// MMIO guard is not supported.
    MmioGuardNotSupported,
    /// Failed to invoke a certain KVM HVC function.
    KvmError(KvmError, u32),
    #[cfg(target_arch = "aarch64")]
    /// Failed to invoke GenieZone HVC function.
    GeniezoneError(GeniezoneError, u32),
    #[cfg(target_arch = "aarch64")]
    /// Unsupported Hypervisor
    UnsupportedHypervisorUuid(Uuid),
    #[cfg(target_arch = "x86_64")]
    /// Unsupported x86_64 Hypervisor
    UnsupportedHypervisor(u128),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::MmioGuardNotSupported => write!(f, "MMIO guard is not supported"),
            Self::KvmError(e, function_id) => {
                write!(f, "Failed to invoke the HVC function with function ID {function_id}: {e}")
            }
            #[cfg(target_arch = "aarch64")]
            Self::GeniezoneError(e, function_id) => {
                write!(
                    f,
                    "Failed to invoke GenieZone HVC function with function ID {function_id}: {e}"
                )
            }
            #[cfg(target_arch = "aarch64")]
            Self::UnsupportedHypervisorUuid(u) => {
                write!(f, "Unsupported Hypervisor UUID {u}")
            }
            #[cfg(target_arch = "x86_64")]
            Self::UnsupportedHypervisor(c) => {
                write!(f, "Unsupported x86_64 Hypervisor {c}")
            }
        }
    }
}
