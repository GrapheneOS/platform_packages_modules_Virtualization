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

//! A client for trusty security VMs during early boot.

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    CpuTopology::CpuTopology, IVirtualizationService::IVirtualizationService,
    VirtualMachineConfig::VirtualMachineConfig, VirtualMachineRawConfig::VirtualMachineRawConfig,
};
use android_system_virtualizationservice::binder::{ParcelFileDescriptor, Strong};
use anyhow::{Context, Result};
use clap::Parser;
use std::fs::File;
use std::path::PathBuf;
use vmclient::VmInstance;

#[derive(Parser)]
/// Collection of CLI for trusty_security_vm_launcher
pub struct Args {
    /// Path to the trusty kernel image.
    #[arg(long)]
    kernel: PathBuf,

    /// Whether the VM is protected or not.
    #[arg(long)]
    protected: bool,

    /// Name of the VM. Used to pull correct config from early_vms.xml
    #[arg(long, default_value = "trusty_security_vm_launcher")]
    name: String,

    /// Memory size of the VM in MiB
    #[arg(long, default_value_t = 128)]
    memory_size_mib: i32,

    /// CPU Topology exposed to the VM <one-cpu|match-host>
    #[arg(long, default_value = "one-cpu", value_parser = parse_cpu_topology)]
    cpu_topology: CpuTopology,
}

fn get_service() -> Result<Strong<dyn IVirtualizationService>> {
    let virtmgr = vmclient::VirtualizationService::new_early()
        .context("Failed to spawn VirtualizationService")?;
    virtmgr.connect().context("Failed to connect to VirtualizationService")
}

fn parse_cpu_topology(s: &str) -> Result<CpuTopology, String> {
    match s {
        "one-cpu" => Ok(CpuTopology::ONE_CPU),
        "match-host" => Ok(CpuTopology::MATCH_HOST),
        _ => Err(format!("Invalid cpu topology {}", s)),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let service = get_service()?;

    let kernel =
        File::open(&args.kernel).with_context(|| format!("Failed to open {:?}", &args.kernel))?;

    let vm_config = VirtualMachineConfig::RawConfig(VirtualMachineRawConfig {
        name: args.name.to_owned(),
        kernel: Some(ParcelFileDescriptor::new(kernel)),
        protectedVm: args.protected,
        memoryMib: args.memory_size_mib,
        cpuTopology: args.cpu_topology,
        platformVersion: "~1.0".to_owned(),
        // TODO: add instanceId
        ..Default::default()
    });

    println!("creating VM");
    let vm = VmInstance::create(
        service.as_ref(),
        &vm_config,
        // console_in, console_out, and log will be redirected to the kernel log by virtmgr
        None, // console_in
        None, // console_out
        None, // log
        None, // dump_dt
        None, // callback
    )
    .context("Failed to create VM")?;
    vm.start().context("Failed to start VM")?;

    println!("started trusty_security_vm_launcher VM");
    let death_reason = vm.wait_for_death();
    eprintln!("trusty_security_vm_launcher ended: {:?}", death_reason);

    // TODO(b/331320802): we may want to use android logger instead of stdio_to_kmsg?

    Ok(())
}
