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

use anyhow::anyhow;
use avflog::LogResult;
use binder::{ExceptionCode, IntoBinderResult, SpIBinder};
use libc::VMADDR_PORT_ANY;
use log::info;
use rpc_servicemanager_aidl::aidl::android::os::IRpcProvider::Vsock::Vsock;
use rpcbinder::RpcServer;

pub fn get_service_connection_info(_name: &str, _cid: u32) -> binder::Result<Vsock> {
    Err(anyhow!("No supported host services"))
        .with_log()
        .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
}

// TODO Supported service added in the next commit
#[allow(dead_code)]
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
