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

//! IAcessor implementation.
//! TODO: Keep this in proper places, so other pVMs can use this.
//! TODO: Allows to customize VMs for launching. (e.g. port, ...)

use android_os_accessor::aidl::android::os::IAccessor::IAccessor;
use binder::{self, Interface, ParcelFileDescriptor};
use log::info;
use std::time::Duration;
use vmclient::VmInstance;

// Note: Do not use LazyServiceGuard here, to make this service and VM are quit
//       when nobody references it.
// TODO(b/353492849): Do not use IAccessor directly.
#[derive(Debug)]
pub struct Accessor {
    // Note: we can't simply keep reference by specifying lifetime to Accessor,
    //       because 'trait Interface' requires 'static.
    vm: VmInstance,
    port: i32,
    instance: String,
}

impl Accessor {
    pub fn new(vm: VmInstance, port: i32, instance: &str) -> Self {
        Self { vm, port, instance: instance.into() }
    }
}

impl Interface for Accessor {}

impl IAccessor for Accessor {
    fn addConnection(&self) -> binder::Result<ParcelFileDescriptor> {
        self.vm.wait_until_ready(Duration::from_secs(20)).unwrap();

        info!("VM is ready. Connecting to service via port {}", self.port);

        self.vm.vm.connectVsock(self.port)
    }
    fn getInstanceName(&self) -> binder::Result<String> {
        Ok(self.instance.clone())
    }
}
