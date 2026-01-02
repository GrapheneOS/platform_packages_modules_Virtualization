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

//! Linux VM Manager

use android_system_virtualizationcommon_non_microdroid::aidl::android::system::virtualizationcommon::IGuestAgent::{
    BnGuestAgent, IGuestAgent,
};
use android_system_virtualmachineservice_non_microdroid::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use anyhow::{Context, Result};
use binder::{BinderFeatures, Interface, Strong};
use rpcbinder::RpcServer;
use rpcbinder::RpcSession;
use std::process::Command;
use vsock::{VMADDR_CID_ANY, VMADDR_CID_HOST};

const GUEST_AGENT_SERVICE_PORT: u32 = 4000;

/// Implementation of `IGuestAgent`
#[derive(Debug, Default)]
struct GuestAgent {}

impl Interface for GuestAgent {}

impl IGuestAgent for GuestAgent {
    fn shutdownAsync(&self) -> binder::Result<()> {
        let status = Command::new("poweroff").status().map_err(|e| {
            binder::Status::new_service_specific_error_str(
                -1,
                Some(format!("Failed to execute poweroff: {}", e)),
            )
        })?;

        if !status.success() {
            return Err(binder::Status::new_service_specific_error_str(
                -1,
                Some(format!("poweroff command failed with status: {}", status)),
            ));
        }

        Ok(())
    }
}

impl GuestAgent {
    fn new_binder() -> Strong<dyn IGuestAgent> {
        BnGuestAgent::new_binder(GuestAgent {}, BinderFeatures::default())
    }
}

fn get_vms_rpc_binder() -> Result<Strong<dyn IVirtualMachineService>> {
    let port = vsock::get_local_cid().context("Could not determine local CID")?;
    let session = RpcSession::new();
    session.set_max_incoming_threads(1);
    session
        .setup_vsock_client(VMADDR_CID_HOST, port)
        .context("Could not connect to IVirtualMachineService")
}

fn main() -> Result<()> {
    let service = get_vms_rpc_binder().context("Failed to connect to VirtualizationService")?;
    let guest_agent = GuestAgent::new_binder();

    let (server, _) =
        RpcServer::new_vsock(guest_agent.as_binder(), VMADDR_CID_ANY, GUEST_AGENT_SERVICE_PORT)?;
    service.registerGuestAgent(&guest_agent).context("Failed to register GuestAgent")?;
    println!("linux_vm_manager started and registered");
    server.join();
    Ok(())
}
