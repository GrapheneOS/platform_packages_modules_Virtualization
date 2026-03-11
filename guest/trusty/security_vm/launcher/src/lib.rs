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

//! Library that exposes run_vm api to launch a vm

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    CpuOptions::CpuOptions, CpuOptions::CpuTopology::CpuTopology,
    IVirtualizationService::IVirtualizationService, VirtualMachineConfig::VirtualMachineConfig,
    VirtualMachineRawConfig::VirtualMachineRawConfig,
};
use android_system_virtualizationservice::binder::{ParcelFileDescriptor, Strong};
use anyhow::{ensure, Context, Result};
use hypervisor_props::is_protected_vm_supported;
use log::{info, warn};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use vmclient::VmInstance;

const INSTANCE_ID_SIZE: usize = 64;

/// Config parameters to launch vm
pub struct VmConfig {
    /// Path to the trusty kernel image.
    pub kernel: PathBuf,

    /// Whether the kernel should be loaded as a bootloader
    pub load_kernel_as_bootloader: bool,

    /// Whether the VM is protected or not.
    pub protected: bool,

    /// Name of the VM
    pub name: String,

    /// Memory size of the VM in MiB
    pub memory_size_mib: i32,

    /// CPU Topology exposed to the VM <one-cpu|match-host>
    pub cpu_topology: CpuTopology,

    /// Custom VM firmware to use (test & development only)
    pub custom_pvmfw: Option<PathBuf>,

    /// tee_services to initialize
    pub tee_services: Vec<String>,

    /// Path to a file containing the VM instance ID.
    pub vm_instance_id: Option<PathBuf>,

    /// File to stream console output.
    pub console_out: Option<File>,

    /// File to stream logs.
    pub log: Option<File>,
}

/// Runs the vm and returns a VmInstance if successful
pub fn run_vm(config: VmConfig) -> Result<VmInstance> {
    let service = get_service()?;

    let kernel = File::open(&config.kernel)
        .with_context(|| format!("Failed to open {:?}", &config.kernel))?;
    let kernel = ParcelFileDescriptor::new(kernel);

    // If --load-kernel-as-bootloader option is present, then load kernel as bootloader
    let (kernel, bootloader) =
        if config.load_kernel_as_bootloader { (None, Some(kernel)) } else { (Some(kernel), None) };

    let protected_vm = if is_protected_vm_supported().unwrap_or(false) {
        config.protected
    } else {
        if config.protected {
            warn!("protected VM is not supported; launch non-protected VM");
        }
        false
    };

    let custom_pvmfw = if let Some(path) = config.custom_pvmfw {
        let file = File::open(&path).with_context(|| format!("Failed to open {path:?}"))?;
        Some(ParcelFileDescriptor::new(file))
    } else {
        None
    };

    let instance_id = if let Some(path) = config.vm_instance_id.as_ref() {
        info!("Loading VM Instance ID from file: {path:?}");
        load_instance_id(path)?
    } else {
        warn!("No VM Instance ID file provided. Using default instance ID.");
        [0u8; INSTANCE_ID_SIZE]
    };

    let vm_config = VirtualMachineConfig::RawConfig(VirtualMachineRawConfig {
        name: config.name.to_owned(),
        kernel,
        bootloader,
        protectedVm: protected_vm,
        customPvmfw: custom_pvmfw,
        memoryMib: config.memory_size_mib,
        cpuOptions: CpuOptions { cpuTopology: config.cpu_topology },
        platformVersion: "~1.0".to_owned(),
        teeServices: config.tee_services,
        instanceId: instance_id,
        ..Default::default()
    });

    info!("creating VM with config {:?}", &vm_config);

    let vm = VmInstance::create(
        service.as_ref(),
        &vm_config,
        // console_in, console_out, and log will be redirected to the kernel log by virtmgr
        config.console_out,
        None,
        config.log,
        None,
    )
    .context("Failed to create VM")?;
    vm.vm.start().context("Failed to start VM")?;
    info!("started VM");

    Ok(vm)
}

fn get_service() -> Result<Strong<dyn IVirtualizationService>> {
    let virtmgr = vmclient::VirtualizationService::new_early()
        .context("Failed to spawn VirtualizationService")?;
    virtmgr.connect().context("Failed to connect to VirtualizationService")
}

fn load_instance_id(path: &Path) -> Result<[u8; INSTANCE_ID_SIZE]> {
    let mut file =
        File::open(path).with_context(|| format!("open VM Instance ID file: {:?}", path))?;

    let metadata = file
        .metadata()
        .with_context(|| format!("get metadata for VM Instance ID file: {:?}", path))?;

    ensure!(
        metadata.len() == INSTANCE_ID_SIZE as u64,
        "VM Instance ID file {:?} has incorrect size. Expected {}, Got {}",
        path,
        INSTANCE_ID_SIZE,
        metadata.len()
    );

    let mut buffer = [0u8; INSTANCE_ID_SIZE];
    file.read_exact(&mut buffer)
        .with_context(|| format!("read VM Instance ID file: {:?}", path))?;
    Ok(buffer)
}
