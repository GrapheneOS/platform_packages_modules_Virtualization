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

//! Re-exports AIDL types commonly used in this crate.

pub use android_os_permissions_aidl::aidl::android::os::{
    IPermissionController::IPermissionController,
};
pub use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::{
    Certificate::Certificate,
    DeathReason::DeathReason,
    ErrorCode::ErrorCode,
    IEncryptedStoreKEK::{BnEncryptedStoreKEK, IEncryptedStoreKEK},
};
pub use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    AssignableDevice::AssignableDevice,
    AssignedDevices::AssignedDevices,
    CpuOptions::CpuOptions,
    CpuOptions::CpuTopology::CpuTopology,
    DiskImage::DiskImage,
    IVirtualMachine::{IVirtualMachine, BnVirtualMachine, ERROR_UNEXPECTED},
    IVirtualMachineCallback::IVirtualMachineCallback,
    IVirtualizationService::{BnVirtualizationService, IVirtualizationService},
    InputDevice::InputDevice,
    Partition::Partition,
    PartitionType::PartitionType,
    SharedPath::SharedPath,
    VirtualMachineAppConfig::{DebugLevel::DebugLevel, Payload::Payload, VirtualMachineAppConfig},
    VirtualMachineConfig::VirtualMachineConfig,
    VirtualMachineDebugInfo::VirtualMachineDebugInfo,
    VirtualMachinePayloadConfig::VirtualMachinePayloadConfig,
    VirtualMachineRawConfig::VirtualMachineRawConfig,
    VirtualMachineState::VirtualMachineState,
};
pub use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::{
    IGuestAgent::IGuestAgent,
    IVirtualMachineService::{BnVirtualMachineService, IVirtualMachineService},
};
pub use android_system_virtualizationservice_internal::aidl::android::system::virtualizationservice_internal::{
    AtomVmBooted::AtomVmBooted,
    AtomVmCreationRequested::AtomVmCreationRequested,
    AtomVmExited::AtomVmExited,
    IVirtualizationServiceInternal::IVirtualizationServiceInternal,
};
pub use android_hardware_security_secretkeeper::aidl::android::hardware::security::secretkeeper::{
    ISecretkeeper::{BnSecretkeeper, ISecretkeeper},
    SecretId::SecretId,
    PublicKey::PublicKey,
};
pub use android_hardware_security_authgraph::aidl::android::hardware::security::authgraph::{
    Arc::Arc,
    IAuthGraphKeyExchange::{BnAuthGraphKeyExchange, IAuthGraphKeyExchange},
    Identity::Identity,
    KeInitResult::KeInitResult,
    Key::Key,
    PubKey::PubKey,
    SessionIdSignature::SessionIdSignature,
    SessionInfo::SessionInfo,
    SessionInitiationInfo::SessionInitiationInfo,
};
