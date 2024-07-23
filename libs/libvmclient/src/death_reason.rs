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

use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::DeathReason::DeathReason as AidlDeathReason;

/// The reason why a VM died.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeathReason {
    /// VirtualizationService died.
    VirtualizationServiceDied,
    /// There was an error waiting for the VM.
    InfrastructureError,
    /// The VM was killed.
    Killed,
    /// The VM died for an unknown reason.
    Unknown,
    /// The VM requested to shut down.
    Shutdown,
    /// crosvm had an error starting the VM.
    StartFailed,
    /// The VM requested to reboot, possibly as the result of a kernel panic.
    Reboot,
    /// The VM or crosvm crashed.
    Crash,
    /// The pVM firmware failed to verify the VM because the public key doesn't match.
    PvmFirmwarePublicKeyMismatch,
    /// The pVM firmware failed to verify the VM because the instance image changed.
    PvmFirmwareInstanceImageChanged,
    /// The microdroid failed to connect to VirtualizationService's RPC server.
    MicrodroidFailedToConnectToVirtualizationService,
    /// The payload for microdroid is changed.
    MicrodroidPayloadHasChanged,
    /// The microdroid failed to verify given payload APK.
    MicrodroidPayloadVerificationFailed,
    /// The VM config for microdroid is invalid (e.g. missing tasks).
    MicrodroidInvalidPayloadConfig,
    /// There was a runtime error while running microdroid manager.
    MicrodroidUnknownRuntimeError,
    /// The VM was killed due to hangup.
    Hangup,
    /// VirtualizationService sent a death reason which was not recognised by the client library.
    Unrecognised(AidlDeathReason),
}

impl From<AidlDeathReason> for DeathReason {
    fn from(reason: AidlDeathReason) -> Self {
        match reason {
            AidlDeathReason::INFRASTRUCTURE_ERROR => Self::InfrastructureError,
            AidlDeathReason::KILLED => Self::Killed,
            AidlDeathReason::UNKNOWN => Self::Unknown,
            AidlDeathReason::SHUTDOWN => Self::Shutdown,
            AidlDeathReason::START_FAILED => Self::StartFailed,
            AidlDeathReason::REBOOT => Self::Reboot,
            AidlDeathReason::CRASH => Self::Crash,
            AidlDeathReason::PVM_FIRMWARE_PUBLIC_KEY_MISMATCH => Self::PvmFirmwarePublicKeyMismatch,
            AidlDeathReason::PVM_FIRMWARE_INSTANCE_IMAGE_CHANGED => {
                Self::PvmFirmwareInstanceImageChanged
            }
            AidlDeathReason::MICRODROID_FAILED_TO_CONNECT_TO_VIRTUALIZATION_SERVICE => {
                Self::MicrodroidFailedToConnectToVirtualizationService
            }
            AidlDeathReason::MICRODROID_PAYLOAD_HAS_CHANGED => Self::MicrodroidPayloadHasChanged,
            AidlDeathReason::MICRODROID_PAYLOAD_VERIFICATION_FAILED => {
                Self::MicrodroidPayloadVerificationFailed
            }
            AidlDeathReason::MICRODROID_INVALID_PAYLOAD_CONFIG => {
                Self::MicrodroidInvalidPayloadConfig
            }
            AidlDeathReason::MICRODROID_UNKNOWN_RUNTIME_ERROR => {
                Self::MicrodroidUnknownRuntimeError
            }
            AidlDeathReason::HANGUP => Self::Hangup,
            _ => Self::Unrecognised(reason),
        }
    }
}
