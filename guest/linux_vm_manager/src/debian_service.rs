// Copyright 2026, The Android Open Source Project
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
use debian_aidl_interface::{
    aidl::com::android::virtualization::debian::aidl::{
        IDebianService::{BnDebianService, IDebianService, VSOCK_PORT},
        IVmActivePortListener::ActivePort::ActivePort,
        IVmActivePortListener::IVmActivePortListener,
    },
    binder::{BinderFeatures, Interface, Result as BinderResult, Status, Strong},
};
use log::{error, warn};
use rpcbinder::RpcServer;
use std::future::Future;
use tokio::runtime::Runtime;
use vsock::VMADDR_CID_ANY;

pub struct DebianService {
    rt: Runtime,
}

impl Interface for DebianService {}

impl DebianService {
    pub fn new_rpc_server() -> RpcServer {
        let rt = create_tokio_runtime();
        let service = DebianService { rt };

        let binder = BnDebianService::new_binder(service, BinderFeatures::default());

        let vsock_port = VSOCK_PORT.try_into().unwrap();
        let (server, _) = RpcServer::new_vsock(binder.as_binder(), VMADDR_CID_ANY, vsock_port)
            .expect("Failed to start debian service rpc server");
        server.set_max_threads(4);
        server
    }
}

fn create_tokio_runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime")
}

// To workaround two Rust compiler errors:
//   - higher-ranked life time error
//   - Implementation of Send is not general enough
fn force_send<F: Future + Send>(f: F) -> impl Future<Output = F::Output> + Send {
    f
}

impl IDebianService for DebianService {
    fn setVmActivePortListener(
        &self,
        listener: &Strong<dyn IVmActivePortListener>,
    ) -> BinderResult<()> {
        let listener = listener.clone();
        self.rt.spawn(async move {
            let ret =
                force_send(forwarder_guest_launcher::monitor_active_ports(async move |ports| {
                    let ports: Vec<ActivePort> = ports
                        .iter()
                        .map(|(port, comm)| ActivePort {
                            port: *port as i32,
                            comm: comm.to_string(),
                        })
                        .collect();

                    listener
                        .reportActivePorts(&ports)
                        .map_err(|e| anyhow!("Error in reportActivePorts(), {e:?}"))
                }))
                .await;

            match ret {
                Ok(()) => warn!("monitor_active_ports is unexpectedly returned"),
                Err(e) => warn!(
                    "monitor_active_ports is returned with error, e={e:?}. May be shutting down."
                ),
            };
        });

        Ok(())
    }

    fn requestForwarding(&self, guest_tcp_port: i32, vsock_port: i32) -> BinderResult<()> {
        let tcp_port = guest_tcp_port
            .try_into()
            .expect("Failed to call requestForwarding(): {guest_tcp_port} out of range");
        forwarder_guest_launcher::forward_port(tcp_port, vsock_port as u32);
        Ok(())
    }

    fn requestStorageBalloon(&self, available_bytes: i64) -> BinderResult<()> {
        let available_bytes = available_bytes.try_into().unwrap();
        storage_balloon_agent::do_storage_ballooning(available_bytes).map_err(|e| {
            error!("Error in storage_balloon_agent(), {e:?}");
            Status::new_service_specific_error(-1, None)
        })
    }
}
