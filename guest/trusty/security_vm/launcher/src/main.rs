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
    CpuOptions::CpuOptions, CpuOptions::CpuTopology::CpuTopology,
    IVirtualizationService::IVirtualizationService, VirtualMachineConfig::VirtualMachineConfig,
    VirtualMachineRawConfig::VirtualMachineRawConfig,
};
use android_system_virtualizationservice::binder::{
    self, ParcelFileDescriptor, ProcessState, Strong,
};
use anyhow::{bail, Context, Result};
use clap::Parser;
use hypervisor_props::is_protected_vm_supported;
use std::fs::File;
use std::path::PathBuf;
use vmclient::VmInstance;

const ACCESSOR_SERVICE_NAME: &str = "android.os.IAccessor/ICommService/default";
const INTERNAL_RPC_SERVICE_NAME: &str = "android.keymint.trusty.commservice.ICommService/default";

#[derive(Parser)]
/// Collection of CLI for trusty_security_vm_launcher
pub struct Args {
    /// Path to the trusty kernel image.
    #[arg(long)]
    kernel: PathBuf,

    // Whether the kernel should be loaded as a bootloader
    #[arg(long)]
    load_kernel_as_bootloader: bool,

    /// Whether the VM is protected or not.
    #[arg(long)]
    protected: bool,

    /// Name of the VM. Used to pull correct config from early_vms.xml
    #[arg(long, default_value = "trusty_security_vm_launcher")]
    name: String,

    /// Memory size of the VM in MiB
    #[arg(long, default_value_t = 128)]
    memory_size_mib: i32,

    /// Port number of the RPC service exposed in VM
    #[arg(long, default_value_t = -1)]
    port: i32,

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
        "one-cpu" => Ok(CpuTopology::CpuCount(1)),
        "match-host" => Ok(CpuTopology::MatchHost(true)),
        _ => Err(format!("Invalid cpu topology {}", s)),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let service = get_service()?;

    let kernel =
        File::open(&args.kernel).with_context(|| format!("Failed to open {:?}", &args.kernel))?;
    let kernel = ParcelFileDescriptor::new(kernel);

    // If --load-kernel-as-bootloader option is present, then load kernel as bootloader
    let (kernel, bootloader) =
        if args.load_kernel_as_bootloader { (None, Some(kernel)) } else { (Some(kernel), None) };

    let protected_vm = if is_protected_vm_supported().unwrap_or(false) {
        args.protected
    } else {
        if args.protected {
            println!("protected VM is not supported; launch non-protected VM");
        }
        false
    };

    let vm_config = VirtualMachineConfig::RawConfig(VirtualMachineRawConfig {
        name: args.name.to_owned(),
        kernel,
        bootloader,
        protectedVm: protected_vm,
        memoryMib: args.memory_size_mib,
        cpuOptions: CpuOptions { cpuTopology: args.cpu_topology },
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
    )
    .context("Failed to create VM")?;
    vm.start(None /* callback */).context("Failed to start VM")?;

    println!("started {} VM", args.name.to_owned());

    if args.port > 0 {
        ProcessState::start_thread_pool();
        let accessor = vm
            .vm
            .createAccessorBinder(INTERNAL_RPC_SERVICE_NAME, args.port)
            .context("failed to create accessor binder")?;
        let accessor_delegator = binder::delegate_accessor(INTERNAL_RPC_SERVICE_NAME, accessor)
            .context("failed to delegate accessor")?;
        // TODO(b/429217397): Use a proper way to register an accessor.
        binder::add_service(ACCESSOR_SERVICE_NAME, accessor_delegator)
            .context("failed to add accessor service")?;
        println!("registered accessor service {ACCESSOR_SERVICE_NAME}");
        ProcessState::join_thread_pool();

        bail!("Thread pool unexpectedly ended");
    } else {
        let death_reason = vm.wait_for_death();
        eprintln!("{} ended: {:?}", args.name.to_owned(), death_reason);
        Ok(())
    }
    // TODO(b/331320802): we may want to use android logger instead of stdio_to_kmsg?
}
