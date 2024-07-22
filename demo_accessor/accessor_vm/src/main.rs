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

//! VM with the simplest service for IAccessor demo

use anyhow::Result;
use com_android_virt_accessor_demo_vm_service::{
    aidl::com::android::virt::accessor_demo::vm_service::IAccessorVmService::{
        BnAccessorVmService, IAccessorVmService,
    },
    binder::{self, BinderFeatures, Interface, Strong},
};
use log::{error, info};

// Private contract between IAccessor impl and VM service.
const PORT: u32 = 5678;

vm_payload::main!(main);

// Entry point of the Service VM client.
fn main() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("accessor_vm")
            .with_max_level(log::LevelFilter::Debug),
    );
    if let Err(e) = try_main() {
        error!("failed with {:?}", e);
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    info!("Starting stub payload for IAccessor demo");

    vm_payload::run_single_vsock_service(AccessorVmService::new_binder(), PORT)
}

struct AccessorVmService {}

impl Interface for AccessorVmService {}

impl AccessorVmService {
    fn new_binder() -> Strong<dyn IAccessorVmService> {
        BnAccessorVmService::new_binder(AccessorVmService {}, BinderFeatures::default())
    }
}

impl IAccessorVmService for AccessorVmService {
    fn add(&self, a: i32, b: i32) -> binder::Result<i32> {
        Ok(a + b)
    }
}
