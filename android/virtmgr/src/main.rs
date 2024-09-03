// Copyright 2022, The Android Open Source Project
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

//! Android Virtualization Manager

mod aidl;
mod atom;
mod composite;
mod crosvm;
mod debug_config;
mod dt_overlay;
mod payload;
mod selinux;

use crate::aidl::{GLOBAL_SERVICE, VirtualizationService};
use android_system_virtualizationservice::aidl::android::system::virtualizationservice::IVirtualizationService::BnVirtualizationService;
use anyhow::{bail, Result};
use binder::{BinderFeatures, ProcessState};
use log::{info, LevelFilter};
use rpcbinder::{FileDescriptorTransportMode, RpcServer};
use std::os::unix::io::{AsFd, RawFd};
use std::sync::LazyLock;
use clap::Parser;
use nix::unistd::{write, Pid, Uid};
use std::os::unix::raw::{pid_t, uid_t};
use safe_ownedfd::take_fd_ownership;

const LOG_TAG: &str = "virtmgr";

static PID_CURRENT: LazyLock<Pid> = LazyLock::new(Pid::this);
static PID_PARENT: LazyLock<Pid> = LazyLock::new(Pid::parent);
static UID_CURRENT: LazyLock<Uid> = LazyLock::new(Uid::current);

fn get_this_pid() -> pid_t {
    // Return the process ID of this process.
    PID_CURRENT.as_raw()
}

fn get_calling_pid() -> pid_t {
    // The caller is the parent of this process.
    PID_PARENT.as_raw()
}

fn get_calling_uid() -> uid_t {
    // The caller and this process share the same UID.
    UID_CURRENT.as_raw()
}

#[derive(Parser)]
struct Args {
    /// File descriptor inherited from the caller to run RpcBinder server on.
    /// This should be one end of a socketpair() compatible with RpcBinder's
    /// UDS bootstrap transport.
    #[clap(long)]
    rpc_server_fd: RawFd,
    /// File descriptor inherited from the caller to signal RpcBinder server
    /// readiness. This should be one end of pipe() and the caller should be
    /// waiting for HUP on the other end.
    #[clap(long)]
    ready_fd: RawFd,
}

fn check_vm_support() -> Result<()> {
    if hypervisor_props::is_any_vm_supported()? {
        Ok(())
    } else {
        // This should never happen, it indicates a misconfigured device where the virt APEX
        // is present but VMs are not supported. If it does happen, fail fast to avoid wasting
        // resources trying.
        bail!("Device doesn't support protected or non-protected VMs")
    }
}

fn main() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag(LOG_TAG)
            .with_max_level(LevelFilter::Info)
            .with_log_buffer(android_logger::LogId::System),
    );

    check_vm_support().unwrap();

    let args = Args::parse();

    let rpc_server_fd =
        take_fd_ownership(args.rpc_server_fd).expect("Failed to take ownership of rpc_server_fd");
    let ready_fd = take_fd_ownership(args.ready_fd).expect("Failed to take ownership of ready_fd");

    // Start thread pool for kernel Binder connection to VirtualizationServiceInternal.
    ProcessState::start_thread_pool();

    if cfg!(early) {
        panic!("Early VM not implemented");
    } else {
        GLOBAL_SERVICE.removeMemlockRlimit().expect("Failed to remove memlock rlimit");
    }

    let service = VirtualizationService::init();
    let service =
        BnVirtualizationService::new_binder(service, BinderFeatures::default()).as_binder();

    let server = RpcServer::new_unix_domain_bootstrap(service, rpc_server_fd)
        .expect("Failed to start RpcServer");
    server.set_supported_file_descriptor_transport_modes(&[FileDescriptorTransportMode::Unix]);

    info!("Started VirtualizationService RpcServer. Ready to accept connections");

    // Signal readiness to the caller by closing our end of the pipe.
    write(ready_fd.as_fd(), "o".as_bytes())
        .expect("Failed to write a single character through ready_fd");
    drop(ready_fd);

    server.join();
    info!("Shutting down VirtualizationService RpcServer");
}
