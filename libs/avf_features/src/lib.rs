// Copyright 2024, The Android Open Source Project
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

//! Provide functionality for handling AVF build-time feature flags.

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    IVirtualizationService::FEATURE_DICE_CHANGES, IVirtualizationService::FEATURE_LLPVM_CHANGES,
    IVirtualizationService::FEATURE_MULTI_TENANT, IVirtualizationService::FEATURE_NETWORK,
    IVirtualizationService::FEATURE_REMOTE_ATTESTATION,
    IVirtualizationService::FEATURE_VENDOR_MODULES,
};
use log::warn;

/// Check if an AVF feature is enabled.
pub fn is_feature_enabled(feature: &str) -> bool {
    match feature {
        FEATURE_DICE_CHANGES => cfg!(dice_changes),
        FEATURE_LLPVM_CHANGES => cfg!(llpvm_changes),
        FEATURE_MULTI_TENANT => cfg!(multi_tenant),
        FEATURE_NETWORK => cfg!(network),
        FEATURE_REMOTE_ATTESTATION => cfg!(remote_attestation),
        FEATURE_VENDOR_MODULES => cfg!(vendor_modules),
        _ => {
            warn!("unknown feature {feature}");
            false
        }
    }
}
