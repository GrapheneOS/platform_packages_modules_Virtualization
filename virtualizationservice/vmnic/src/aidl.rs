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

//! Implementation of the AIDL interface of Vmnic.

use anyhow::anyhow;
use android_system_virtualizationservice_internal::aidl::android::system::virtualizationservice_internal::IVmnic::IVmnic;
use binder::{self, ExceptionCode, Interface, IntoBinderResult, ParcelFileDescriptor};
use log::info;

#[derive(Debug, Default)]
pub struct Vmnic {}

impl Vmnic {
    pub fn init() -> Vmnic {
        Vmnic::default()
    }
}

impl Interface for Vmnic {}

impl IVmnic for Vmnic {
    fn createTapInterface(&self, iface_name_suffix: &str) -> binder::Result<ParcelFileDescriptor> {
        let ifname = format!("avf_tap_{iface_name_suffix}");
        info!("Creating TAP interface {}", ifname);

        Err(anyhow!("Creating TAP network interface is not supported yet"))
            .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }
}
