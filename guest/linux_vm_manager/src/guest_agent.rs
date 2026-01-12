// Copyright 2026, The Android Open Source Project
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

use android_system_virtualizationcommon_non_microdroid::{
    aidl::android::system::virtualizationcommon::IGuestAgent::{BnGuestAgent, IGuestAgent},
    binder::{BinderFeatures, Interface, Result as BinderResult, Status, Strong},
};
use log::error;

pub struct GuestAgent {}

impl Interface for GuestAgent {}

impl GuestAgent {
    pub fn new_binder() -> Strong<dyn IGuestAgent> {
        let guest_agent = GuestAgent {};
        BnGuestAgent::new_binder(guest_agent, BinderFeatures::default())
    }
}

impl IGuestAgent for GuestAgent {
    fn shutdownAsync(&self) -> BinderResult<()> {
        shutdown_runner::power_off().map_err(|e| {
            error!("Error in power_off(), {e:?}");
            Status::new_service_specific_error(-1, None)
        })
    }
}
