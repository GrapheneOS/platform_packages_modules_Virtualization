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

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::CpuOptions::CpuTopology::CpuTopology;
use android_system_virtualizationservice::binder::{
    self, ProcessState,
};
use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use env_logger::Builder;
use log::{error, info, trace, LevelFilter};
use nix::fcntl::OFlag;
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use vmclient::VmInstance;
use vm_launcher::{VmConfig, run_vm};

const GUEST_FFA_TEE_SERVICE: &str = "guest_ffa_tee_service";

#[derive(Parser, Debug)]
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
    #[arg(long, default_value = "security_vm")]
    name: String,

    /// Memory size of the VM in MiB
    #[arg(long, default_value_t = 128)]
    memory_size_mib: i32,

    /// Path to a JSON file defining the RPC services to register.
    #[arg(long, value_name = "FILE")]
    rpc_services_config: Vec<PathBuf>,

    /// CPU Topology exposed to the VM <one-cpu|match-host>
    #[arg(long, default_value = "one-cpu", value_parser = parse_cpu_topology)]
    cpu_topology: CpuTopology,

    /// Custom VM firmware to use (test & development only).
    #[arg(long)]
    custom_pvmfw: Option<PathBuf>,

    /// If enabled, allow this VM to access FF-A. The launching process must
    /// have CAP_IPC_OWNER and be configured by selinux to use guest_ffa_tee_service.
    /// This is only settable on a protected vm (enforced by virtmgr).
    #[arg(long)]
    allow_ffa: bool,

    /// Path to a file containing the VM instance ID.
    #[arg(long, value_name = "FILE")]
    vm_instance_id: Option<PathBuf>,

    /// Whether to use the early virtmgr.
    #[arg(long="no-early", default_value_t = true, action = clap::ArgAction::SetFalse)]
    early: bool,
}

impl Args {
    fn to_vm_config(
        &self,
        console_out: File,
        log_out: File,
        tee_services: Vec<String>,
    ) -> VmConfig {
        VmConfig {
            kernel: self.kernel.clone(),
            load_kernel_as_bootloader: self.load_kernel_as_bootloader,
            protected: self.protected,
            name: self.name.clone(),
            memory_size_mib: self.memory_size_mib,
            cpu_topology: self.cpu_topology.clone(),
            custom_pvmfw: self.custom_pvmfw.clone(),
            tee_services,
            vm_instance_id: self.vm_instance_id.clone(),
            console_out: Some(console_out),
            log: Some(log_out),
            early: self.early,
        }
    }
}

fn parse_cpu_topology(s: &str) -> Result<CpuTopology, String> {
    match s {
        "one-cpu" => Ok(CpuTopology::CpuCount(1)),
        "match-host" => Ok(CpuTopology::MatchHost(true)),
        _ => Err(format!("Invalid cpu topology {s}")),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    const BINARY_NAME: &str = "trusty_security_vm_launcher";
    let vm_name = args.name.to_owned();
    Builder::new()
        // Set the default log level if not configured via RUST_LOG
        .filter_level(LevelFilter::Info)
        .format(move |buf, record| {
            writeln!(
                buf,
                // Format: "[LEVEL] binary_name:vm_name: log_message"
                "[{}] {}:{}: {}",
                record.level(),
                BINARY_NAME,
                vm_name,
                record.args()
            )
        })
        .init();

    let tee_services = match args.allow_ffa {
        true => vec![GUEST_FFA_TEE_SERVICE.to_owned()],
        false => Vec::new(),
    };

    let console_out = create_log_writer(&args.name)?;
    // Creates only one pipe and one thread for efficiency.
    let log_out = console_out.try_clone().context("Failed to clone console_out fd for log_out")?;

    let launch_vm_config = args.to_vm_config(console_out, log_out, tee_services);
    let vm = run_vm(launch_vm_config)?;

    if !args.rpc_services_config.is_empty() {
        ProcessState::start_thread_pool();

        for config_path in args.rpc_services_config {
            let configs = parse_rpc_service_configs(&config_path)?;
            ensure!(
                !configs.is_empty(),
                "RPC services config file at '{:?}' is empty",
                config_path
            );

            info!("Registering {} RPC service(s) from {}...", configs.len(), config_path.display());
            for config in &configs {
                register_accessor_service(&vm, config)?;
            }
        }

        ProcessState::join_thread_pool();
        bail!("Thread pool unexpectedly ended");
    } else {
        info!("No --rpc-services-config provided. Not registering any accessor services.");
        let death_reason = vm.wait_for_death();
        error!("VM ended: {death_reason:?}");
        Ok(())
    }
}

/// Defines the structure of a single RPC service configuration in the JSON file.
#[derive(Deserialize, Debug)]
struct RpcServiceConfig {
    port: i32,
    accessor_name: String,
    internal_rpc_service_name: String,
}

/// Parses a JSON file containing an array of RPC service configurations.
fn parse_rpc_service_configs(path: &Path) -> Result<Vec<RpcServiceConfig>> {
    let file =
        File::open(path).with_context(|| format!("open RPC services config at '{path:?}'"))?;
    serde_json::from_reader(file).with_context(|| format!("parse JSON from '{path:?}'"))
}

fn register_accessor_service(vm: &VmInstance, config: &RpcServiceConfig) -> Result<()> {
    trace!("Registering service '{}' on port {}", &config.accessor_name, config.port);
    let accessor = vm
        .vm
        .createAccessorBinder(&config.internal_rpc_service_name, config.port)
        .with_context(|| format!("create accessor binder for '{config:?}'"))?;
    let accessor_delegator = binder::delegate_accessor(&config.internal_rpc_service_name, accessor)
        .with_context(|| format!("delegate accessor for '{}'", config.accessor_name))?;

    // TODO(b/429217397): Use a proper way to register an accessor.
    binder::add_service(&config.accessor_name, accessor_delegator)
        .with_context(|| format!("add accessor service: {}", config.accessor_name))?;
    info!("Registered service '{}' on port {}", config.accessor_name, config.port);
    Ok(())
}

/// Creates a pipe and spawns a thread to forward the VM's output to stdout.
fn create_log_writer(prefix: &str) -> Result<File> {
    let (reader_fd, writer_fd) =
        nix::unistd::pipe2(OFlag::O_CLOEXEC).context("Failed to create pipe for VM output")?;
    let reader = File::from(reader_fd);
    let writer = File::from(writer_fd);

    std::thread::Builder::new()
        .name(format!("vm-log-{prefix}"))
        .spawn(move || {
            let reader = BufReader::new(reader);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                // Prefix guest logs to distinguish them from launcher logs
                info!("vm: {line}");
            }
        })
        .context("Failed to spawn VM logging thread")?;
    Ok(writer)
}
