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

//! Implementation of the AIDL interface `IVmPayloadService`.

use android_system_virtualization_internal::aidl::android::system::virtualization::internal::IVmInternalService::IVmInternalService;
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use binder::{Interface, Strong};

pub(crate) struct VmInternalService {
    virtual_machine_service: Strong<dyn IVirtualMachineService>,
}

impl IVmInternalService for VmInternalService {
    fn reportAtomFsckFailedToHost(&self, exit_code: i32) -> binder::Result<()> {
        self.virtual_machine_service.atomFsckFailedReported(exit_code)
    }
}
impl Interface for VmInternalService {}

impl VmInternalService {
    pub(crate) fn new(virtual_machine_service: Strong<dyn IVirtualMachineService>) -> Self {
        Self { virtual_machine_service }
    }
}
