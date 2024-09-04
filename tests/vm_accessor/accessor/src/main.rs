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

//! Android VM control tool.

mod accessor;
mod run;

use accessor::Accessor;
use android_os_accessor::aidl::android::os::IAccessor::BnAccessor;
use anyhow::Error;
use anyhow::{anyhow, bail};
use binder::{BinderFeatures, ProcessState};
use log::info;
use run::run_vm;

// Private contract between IAccessor impl and VM service.
const PORT: i32 = 5678;

// MUST match with VINTF and init.rc
// TODO(b/354632613): Get this from VINTF
const SERVICE_NAME: &str = "android.os.IAccessor/IAccessorVmService/default";

fn main() -> Result<(), Error> {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("accessor_demo")
            .with_max_level(log::LevelFilter::Debug),
    );

    let vm = run_vm()?;

    // If you want to serve multiple services in a VM, then register Accessor impls multiple times.
    let accessor = Accessor::new(vm, PORT, SERVICE_NAME);
    let accessor_binder = BnAccessor::new_binder(accessor, BinderFeatures::default());
    binder::register_lazy_service(SERVICE_NAME, accessor_binder.as_binder()).map_err(|e| {
        anyhow!("Failed to register lazy service, service={SERVICE_NAME}, err={e:?}",)
    })?;
    info!("service {SERVICE_NAME} is registered as lazy service");

    ProcessState::join_thread_pool();

    bail!("Thread pool unexpectedly ended")
}
