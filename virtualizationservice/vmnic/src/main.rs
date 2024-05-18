// Copyright 2024 The Android Open Source Project
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

//! Android Vmnic (Virtual Machine Network Interface Creator)

mod aidl;

use crate::aidl::Vmnic;
use android_logger::Config;
use android_system_virtualizationservice_internal::aidl::android::system::virtualizationservice_internal::IVmnic::{
    BnVmnic,
    BpVmnic,
    IVmnic,
};
use binder::{register_lazy_service, BinderFeatures, ProcessState};
use log::{info, LevelFilter};

const LOG_TAG: &str = "Vmnic";

fn main() {
    android_logger::init_once(
        Config::default()
            .with_tag(LOG_TAG)
            .with_max_level(LevelFilter::Info)
            .with_log_buffer(android_logger::LogId::System),
    );

    let service = Vmnic::init();
    let service = BnVmnic::new_binder(service, BinderFeatures::default());
    register_lazy_service(<BpVmnic as IVmnic>::get_descriptor(), service.as_binder()).unwrap();
    info!("Registered Binder service, joining threadpool.");
    ProcessState::join_thread_pool();
}
