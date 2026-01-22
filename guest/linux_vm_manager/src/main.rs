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

mod debian_service;
mod guest_agent;

use guest_agent::GuestAgent;
use debian_aidl_interface::binder::Strong;
use android_system_virtualmachineservice_non_microdroid::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use crate::debian_service::DebianService;
use anyhow::{Context, Result};
use rpcbinder::RpcSession;
use vsock::VMADDR_CID_HOST;
use std::panic;
use std::process::exit;
use log::{error, info};

fn get_vms_rpc_binder() -> Result<Strong<dyn IVirtualMachineService>> {
    let port = vsock::get_local_cid().context("Could not determine local CID")?;
    info!("Starting service with cid={port}");

    let session = RpcSession::new();
    session.set_max_incoming_threads(1);
    session
        .setup_vsock_client(VMADDR_CID_HOST, port)
        .context("Could not connect to IVirtualMachineService")
}

fn main() -> Result<()> {
    env_logger::builder().filter_level(log::LevelFilter::Debug).init();

    // Redirect panic messages to stderr with backtrace
    panic::set_hook(Box::new(|panic_info| {
        error!("Panic: {panic_info}");
        let backtrace = std::backtrace::Backtrace::force_capture();
        error!("Backtrace: {:#?}", backtrace);
        exit(1);
    }));

    let service = get_vms_rpc_binder().context("Failed to connect to VirtualizationService")?;

    let debian_server = DebianService::new_rpc_server();
    let guest_agent = GuestAgent::new_binder();

    service.registerGuestAgent(&guest_agent).context("Failed to register GuestAgent")?;
    info!("linux_vm_manager started and registered");

    debian_server.join();

    info!("linux_vm_manager is shutting down");

    Ok(())
}
