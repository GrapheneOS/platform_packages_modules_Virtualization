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

use android_system_virtualization_internal::aidl::android::system::virtualization::internal::IVmInternalService::{
    BnVmInternalService, IVmInternalService, VM_INTERNAL_SERVICE_SOCKET_NAME,
};
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use anyhow::Result;
use binder::{Interface, BinderFeatures, Strong};
use log::info;
use rpcbinder::RpcServer;
use std::os::unix::io::OwnedFd;

struct VmInternalService {
    virtual_machine_service: Strong<dyn IVirtualMachineService>,
}

impl IVmInternalService for VmInternalService {
    fn writeToHostDropBox(&self, tag: &str, text: &str) -> binder::Result<()> {
        self.virtual_machine_service.writeToDropBox(tag, text)
    }
}

impl Interface for VmInternalService {}

impl VmInternalService {
    fn new(virtual_machine_service: Strong<dyn IVirtualMachineService>) -> Self {
        Self { virtual_machine_service }
    }
}

/// Registers the `IVmInternalService` service.
pub(crate) fn register_vm_internal_service(
    virtual_machine_service: Strong<dyn IVirtualMachineService>,
    vm_internal_service_fd: OwnedFd,
) -> Result<()> {
    let vm_internal_binder = BnVmInternalService::new_binder(
        VmInternalService::new(virtual_machine_service),
        BinderFeatures::default(),
    );

    let server =
        RpcServer::new_bound_socket(vm_internal_binder.as_binder(), vm_internal_service_fd)?;
    info!("The RPC server '{}' is running.", VM_INTERNAL_SERVICE_SOCKET_NAME);

    // Move server reference into a background thread and run it forever.
    std::thread::spawn(move || {
        server.join();
    });
    Ok(())
}
