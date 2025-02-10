// Copyright 2025 The Android Open Source Project
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

//! Tests running an early VM

use android_system_virtualizationservice::{
    aidl::android::system::virtualizationservice::{
        IVirtualizationService::IVirtualizationService, VirtualMachineConfig::VirtualMachineConfig,
        VirtualMachineRawConfig::VirtualMachineRawConfig,
    },
    binder::{ParcelFileDescriptor, ProcessState, Strong},
};
use anyhow::{Context, Result};
use clap::Parser;
use log::info;
use std::fs::File;
use std::path::PathBuf;

use service_vm_comm::{Request, Response, VmType};
use service_vm_manager::ServiceVm;
use vmclient::VmInstance;

const VM_MEMORY_MB: i32 = 16;

#[derive(Parser)]
/// Collection of CLI for avf_early_vm_test_rialto
pub struct Args {
    /// Path to the Rialto kernel image.
    #[arg(long)]
    kernel: PathBuf,

    /// Whether the VM is protected or not.
    #[arg(long)]
    protected: bool,
}

fn get_service() -> Result<Strong<dyn IVirtualizationService>> {
    let virtmgr = vmclient::VirtualizationService::new_early()
        .context("Failed to spawn VirtualizationService")?;
    virtmgr.connect().context("Failed to connect to VirtualizationService")
}

fn main() -> Result<()> {
    if std::env::consts::ARCH != "aarch64" {
        info!("{} not supported. skipping test", std::env::consts::ARCH);
        return Ok(());
    }

    if !cfg!(early_vm_enabled) {
        info!("early VM disabled. skipping test");
        return Ok(());
    }

    let args = Args::parse();

    if args.protected {
        if !hypervisor_props::is_protected_vm_supported()? {
            info!("pVMs are not supported on device. skipping test");
            return Ok(());
        }
    } else if !hypervisor_props::is_vm_supported()? {
        info!("non-pVMs are not supported on device. skipping test");
        return Ok(());
    }

    let service = get_service()?;
    let kernel =
        File::open(&args.kernel).with_context(|| format!("Failed to open {:?}", &args.kernel))?;
    let kernel = ParcelFileDescriptor::new(kernel);

    let vm_config = VirtualMachineConfig::RawConfig(VirtualMachineRawConfig {
        name: "avf_early_vm_test_launcher".to_owned(),
        kernel: Some(kernel),
        protectedVm: args.protected,
        memoryMib: VM_MEMORY_MB,
        platformVersion: "~1.0".to_owned(),
        ..Default::default()
    });

    let vm_instance = VmInstance::create(
        service.as_ref(),
        &vm_config,
        // console_in, console_out, and log will be redirected to the kernel log by virtmgr
        None, // console_in
        None, // console_out
        None, // log
        None, // dump_dt
    )
    .context("Failed to create VM")?;

    ProcessState::start_thread_pool();

    let vm_type = if args.protected { VmType::ProtectedVm } else { VmType::NonProtectedVm };
    let mut vm_service = ServiceVm::start_vm(vm_instance, vm_type)?;

    let request_data = vec![1, 2, 3, 4, 5];
    let reversed_data = vec![5, 4, 3, 2, 1];
    let response = vm_service
        .process_request(Request::Reverse(request_data))
        .context("Failed to process request")?;
    assert_eq!(Response::Reverse(reversed_data), response);

    Ok(())
}
