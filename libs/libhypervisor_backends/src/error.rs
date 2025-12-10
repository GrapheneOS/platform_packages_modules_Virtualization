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

use core::result;

#[cfg(target_arch = "aarch64")]
use super::hypervisor::GeniezoneError;
#[cfg(target_arch = "aarch64")]
use super::hypervisor::GunyahError;
use super::hypervisor::KvmError;
#[cfg(target_arch = "aarch64")]
use uuid::Uuid;

/// Result type with hypervisor error.
pub type Result<T> = result::Result<T, Error>;

/// Hypervisor error.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    /// MMIO guard is not supported.
    #[error("MMIO guard is not supported")]
    MmioGuardNotSupported,
    /// Failed to invoke a certain KVM HVC function.
    #[error("Failed to invoke the HVC function with function ID {1}: {0}")]
    KvmError(KvmError, u32),
    #[cfg(target_arch = "aarch64")]
    /// Failed to invoke GenieZone HVC function.
    #[error("Failed to invoke GenieZone HVC function with function ID {1}: {0}")]
    GeniezoneError(GeniezoneError, u32),
    #[cfg(target_arch = "aarch64")]
    /// Unsupported Hypervisor
    #[error("Unsupported Hypervisor UUID {0}")]
    UnsupportedHypervisorUuid(Uuid),
    #[cfg(target_arch = "x86_64")]
    /// Unsupported x86_64 Hypervisor
    #[error("Unsupported x86_64 Hypervisor {0}")]
    UnsupportedHypervisor(u128),
    #[cfg(target_arch = "aarch64")]
    /// Failed to invoke Gunyah HVC.
    #[error("Failed to invoke Gunyah HVC: {0}")]
    GunyahError(GunyahError),
}
