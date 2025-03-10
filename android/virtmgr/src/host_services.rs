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

use android_frameworks_stats::aidl::android::frameworks::stats::{
    IStats::{BnStats, BpStats, IStats},
    VendorAtom::VendorAtom,
};
use anyhow::{anyhow, Context};
use avflog::LogResult;
use binder::{BinderFeatures, ExceptionCode, Interface, IntoBinderResult, SpIBinder, Strong};
use libc::VMADDR_PORT_ANY;
use log::info;
use rpc_servicemanager_aidl::aidl::android::os::IRpcProvider::Vsock::Vsock;
use rpcbinder::RpcServer;
use std::sync::Mutex;

pub fn get_service_connection_info(name: &str, cid: u32) -> binder::Result<Vsock> {
    // We currently only support the stats service. It can be used as an example
    // to support more host services with this proxy pattern.
    let stats_service_instance = <BpStats as IStats>::get_descriptor().to_owned() + "/default";
    if name != stats_service_instance {
        return Err(anyhow!("Unknown service"))
            .with_log()
            .or_binder_exception(ExceptionCode::SECURITY);
    }

    // don't block this service while attempting to get a service for the client in the VM.
    let local_svc: Strong<dyn IStats> = binder::check_interface(name)
        .context("Failed to get {name} from IVirtualMachineService")
        .or_service_specific_exception(-1)?;

    // Setup a delegator that wraps the service so we can proxy calls.
    // This is needed because we can't use a kernel binder from
    // another process for a new RpcServer.
    let service = BnStats::new_binder(
        StatsDelegator { delegate: local_svc.into() },
        BinderFeatures::default(),
    );

    start_delegator_vsock_service(service.as_binder(), name, cid)
}

// TODO(b/198785815) create a rust/ndk delegator and use that instead of writing our own.
struct StatsDelegator {
    delegate: Mutex<Strong<dyn IStats>>,
}

impl Interface for StatsDelegator {}

impl IStats for StatsDelegator {
    fn reportVendorAtom(&self, atom: &VendorAtom) -> binder::Result<()> {
        self.delegate.lock().unwrap().reportVendorAtom(atom)
    }
}

fn start_delegator_vsock_service(binder: SpIBinder, name: &str, cid: u32) -> binder::Result<Vsock> {
    match RpcServer::new_vsock(binder, cid, VMADDR_PORT_ANY) {
        Ok((vm_server, assigned_port)) => {
            let port: i32 = assigned_port as i32;
            // two threads in case we need callbacks
            vm_server.set_max_threads(2);
            vm_server.start();
            info!("RpcServer proxy started for {name} on port {port}.");
            Ok(Vsock { port, cid: cid as i32 })
        }
        Err(err) => Err(anyhow!("Could not start RpcServer for {name}: {}", err))
            .with_log()
            .or_binder_exception(ExceptionCode::SECURITY),
    }
}
