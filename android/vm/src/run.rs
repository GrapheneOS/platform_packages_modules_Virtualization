// Copyright 2021, The Android Open Source Project
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

//! Command to run a VM.

use crate::create_partition::command_create_partition;
use crate::{get_service, RunAppConfig, RunCustomVmConfig, RunMicrodroidConfig};
use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    IVirtualizationService::IVirtualizationService,
    PartitionType::PartitionType,
    VirtualMachineAppConfig::{
        CustomConfig::CustomConfig, DebugLevel::DebugLevel, Payload::Payload,
        VirtualMachineAppConfig,
    },
    VirtualMachineConfig::VirtualMachineConfig,
    VirtualMachinePayloadConfig::VirtualMachinePayloadConfig,
    VirtualMachineState::VirtualMachineState,
};
use anyhow::{anyhow, bail, Context, Error};
use binder::ParcelFileDescriptor;
use glob::glob;
use microdroid_payload_config::VmPayloadConfig;
use rand::{distributions::Alphanumeric, Rng};
use std::fs;
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use vmclient::{ErrorCode, VmInstance};
use vmconfig::{get_debug_level, open_parcel_file, VmConfig};
use zip::ZipArchive;

/// Run a VM from the given APK, idsig, and config.
pub fn command_run_app(config: RunAppConfig) -> Result<(), Error> {
    let service = get_service()?;
    let apk = File::open(&config.apk).context("Failed to open APK file")?;

    let extra_apks = match config.config_path.as_deref() {
        Some(path) => parse_extra_apk_list(&config.apk, path)?,
        None => config.extra_apks().to_vec(),
    };

    if extra_apks.len() != config.extra_idsigs.len() {
        bail!(
            "Found {} extra apks, but there are {} extra idsigs",
            extra_apks.len(),
            config.extra_idsigs.len()
        )
    }

    for (i, extra_apk) in extra_apks.iter().enumerate() {
        let extra_apk_fd = ParcelFileDescriptor::new(File::open(extra_apk)?);
        let extra_idsig_fd = ParcelFileDescriptor::new(File::create(&config.extra_idsigs[i])?);
        service.createOrUpdateIdsigFile(&extra_apk_fd, &extra_idsig_fd)?;
    }

    let idsig = File::create(&config.idsig).context("Failed to create idsig file")?;

    let apk_fd = ParcelFileDescriptor::new(apk);
    let idsig_fd = ParcelFileDescriptor::new(idsig);
    service.createOrUpdateIdsigFile(&apk_fd, &idsig_fd)?;

    let idsig = File::open(&config.idsig).context("Failed to open idsig file")?;
    let idsig_fd = ParcelFileDescriptor::new(idsig);

    if !config.instance.exists() {
        const INSTANCE_FILE_SIZE: u64 = 10 * 1024 * 1024;
        command_create_partition(
            service.as_ref(),
            &config.instance,
            INSTANCE_FILE_SIZE,
            PartitionType::ANDROID_VM_INSTANCE,
        )?;
    }

    let instance_id = if cfg!(llpvm_changes) {
        let id_file = config.instance_id()?;
        if id_file.exists() {
            let mut id = [0u8; 64];
            let mut instance_id_file = File::open(id_file)?;
            instance_id_file.read_exact(&mut id)?;
            id
        } else {
            let id = service.allocateInstanceId().context("Failed to allocate instance_id")?;
            let mut instance_id_file = File::create(id_file)?;
            instance_id_file.write_all(&id)?;
            id
        }
    } else {
        // if llpvm feature flag is disabled, instance_id is not used.
        [0u8; 64]
    };

    let storage = if let Some(ref path) = config.microdroid.storage {
        if !path.exists() {
            command_create_partition(
                service.as_ref(),
                path,
                config.microdroid.storage_size.unwrap_or(10 * 1024 * 1024),
                PartitionType::ENCRYPTEDSTORE,
            )?;
        }
        Some(open_parcel_file(path, true)?)
    } else {
        None
    };

    let vendor =
        config.microdroid.vendor().as_ref().map(|p| open_parcel_file(p, false)).transpose()?;

    let extra_idsig_files: Result<Vec<_>, _> = config.extra_idsigs.iter().map(File::open).collect();
    let extra_idsig_fds = extra_idsig_files?.into_iter().map(ParcelFileDescriptor::new).collect();

    let payload = if let Some(config_path) = config.config_path {
        if config.payload_binary_name.is_some() {
            bail!("Only one of --config-path or --payload-binary-name can be defined")
        }
        Payload::ConfigPath(config_path)
    } else if let Some(payload_binary_name) = config.payload_binary_name {
        let extra_apk_files: Result<Vec<_>, _> = extra_apks.iter().map(File::open).collect();
        let extra_apk_fds = extra_apk_files?.into_iter().map(ParcelFileDescriptor::new).collect();

        Payload::PayloadConfig(VirtualMachinePayloadConfig {
            payloadBinaryName: payload_binary_name,
            extraApks: extra_apk_fds,
        })
    } else {
        bail!("Either --config-path or --payload-binary-name must be defined")
    };

    let os_name = if let Some(ver) = config.microdroid.gki() {
        format!("microdroid_gki-{ver}")
    } else {
        "microdroid".to_owned()
    };

    let payload_config_str = format!("{:?}!{:?}", config.apk, payload);

    let mut custom_config = CustomConfig {
        gdbPort: config.debug.gdb.map(u16::from).unwrap_or(0) as i32, // 0 means no gdb
        vendorImage: vendor,
        devices: config
            .microdroid
            .devices()
            .iter()
            .map(|x| {
                x.to_str().map(String::from).ok_or(anyhow!("Failed to convert {x:?} to String"))
            })
            .collect::<Result<_, _>>()?,
        networkSupported: config.common.network_supported(),
        ..Default::default()
    };

    if config.debug.enable_earlycon() {
        if config.debug.debug != DebugLevel::FULL {
            bail!("earlycon is only supported for debuggable VMs")
        }
        if cfg!(target_arch = "aarch64") {
            custom_config
                .extraKernelCmdlineParams
                .push(String::from("earlycon=uart8250,mmio,0x3f8"));
        } else if cfg!(target_arch = "x86_64") {
            custom_config.extraKernelCmdlineParams.push(String::from("earlycon=uart8250,io,0x3f8"));
        } else {
            bail!("unexpected architecture!");
        }
    }

    let vm_config = VirtualMachineConfig::AppConfig(VirtualMachineAppConfig {
        name: config.common.name.unwrap_or_else(|| String::from("VmRunApp")),
        apk: apk_fd.into(),
        idsig: idsig_fd.into(),
        extraIdsigs: extra_idsig_fds,
        instanceImage: open_parcel_file(&config.instance, true /* writable */)?.into(),
        instanceId: instance_id,
        encryptedStorageImage: storage,
        payload,
        debugLevel: config.debug.debug,
        protectedVm: config.common.protected,
        memoryMib: config.common.mem.unwrap_or(0) as i32, // 0 means use the VM default
        cpuTopology: config.common.cpu_topology,
        customConfig: Some(custom_config),
        osName: os_name,
        hugePages: config.common.hugepages,
        boostUclamp: config.common.boost_uclamp,
    });
    run(
        service.as_ref(),
        &vm_config,
        &payload_config_str,
        config.debug.console.as_ref().map(|p| p.as_ref()),
        config.debug.console_in.as_ref().map(|p| p.as_ref()),
        config.debug.log.as_ref().map(|p| p.as_ref()),
    )
}

fn find_empty_payload_apk_path() -> Result<PathBuf, Error> {
    const GLOB_PATTERN: &str = "/apex/com.android.virt/app/**/EmptyPayloadApp*.apk";
    let mut entries: Vec<PathBuf> =
        glob(GLOB_PATTERN).context("failed to glob")?.filter_map(|e| e.ok()).collect();
    if entries.len() > 1 {
        return Err(anyhow!("Found more than one apk matching {}", GLOB_PATTERN));
    }
    match entries.pop() {
        Some(path) => Ok(path),
        None => Err(anyhow!("No apks match {}", GLOB_PATTERN)),
    }
}

fn create_work_dir() -> Result<PathBuf, Error> {
    let s: String =
        rand::thread_rng().sample_iter(&Alphanumeric).take(17).map(char::from).collect();
    let work_dir = PathBuf::from("/data/local/tmp/microdroid").join(s);
    println!("creating work dir {}", work_dir.display());
    fs::create_dir_all(&work_dir).context("failed to mkdir")?;
    Ok(work_dir)
}

/// Run a VM with Microdroid
pub fn command_run_microdroid(config: RunMicrodroidConfig) -> Result<(), Error> {
    let apk = find_empty_payload_apk_path()?;
    println!("found path {}", apk.display());

    let work_dir = config.work_dir.unwrap_or(create_work_dir()?);
    let idsig = work_dir.join("apk.idsig");
    println!("apk.idsig path: {}", idsig.display());
    let instance_img = work_dir.join("instance.img");
    println!("instance.img path: {}", instance_img.display());

    let mut app_config = RunAppConfig {
        common: config.common,
        debug: config.debug,
        microdroid: config.microdroid,
        apk,
        idsig,
        instance: instance_img,
        payload_binary_name: Some("MicrodroidEmptyPayloadJniLib.so".to_owned()),
        ..Default::default()
    };

    if cfg!(llpvm_changes) {
        app_config.set_instance_id(work_dir.join("instance_id"))?;
        println!("instance_id file path: {}", app_config.instance_id()?.display());
    }

    command_run_app(app_config)
}

/// Run a VM from the given configuration file.
pub fn command_run(config: RunCustomVmConfig) -> Result<(), Error> {
    let config_file = File::open(&config.config).context("Failed to open config file")?;
    let mut vm_config =
        VmConfig::load(&config_file).context("Failed to parse config file")?.to_parcelable()?;
    if let Some(mem) = config.common.mem {
        vm_config.memoryMib = mem as i32;
    }
    if let Some(name) = config.common.name {
        vm_config.name = name;
    } else {
        vm_config.name = String::from("VmRun");
    }
    if let Some(gdb) = config.debug.gdb {
        vm_config.gdbPort = gdb.get() as i32;
    }
    vm_config.cpuTopology = config.common.cpu_topology;
    vm_config.hugePages = config.common.hugepages;
    vm_config.boostUclamp = config.common.boost_uclamp;
    run(
        get_service()?.as_ref(),
        &VirtualMachineConfig::RawConfig(vm_config),
        &format!("{:?}", &config.config),
        config.debug.console.as_ref().map(|p| p.as_ref()),
        config.debug.console_in.as_ref().map(|p| p.as_ref()),
        config.debug.log.as_ref().map(|p| p.as_ref()),
    )
}

fn state_to_str(vm_state: VirtualMachineState) -> &'static str {
    match vm_state {
        VirtualMachineState::NOT_STARTED => "NOT_STARTED",
        VirtualMachineState::STARTING => "STARTING",
        VirtualMachineState::STARTED => "STARTED",
        VirtualMachineState::READY => "READY",
        VirtualMachineState::FINISHED => "FINISHED",
        VirtualMachineState::DEAD => "DEAD",
        _ => "(invalid state)",
    }
}

fn run(
    service: &dyn IVirtualizationService,
    config: &VirtualMachineConfig,
    payload_config: &str,
    console_out_path: Option<&Path>,
    console_in_path: Option<&Path>,
    log_path: Option<&Path>,
) -> Result<(), Error> {
    let console_out = if let Some(console_out_path) = console_out_path {
        Some(File::create(console_out_path).with_context(|| {
            format!("Failed to open console output file {:?}", console_out_path)
        })?)
    } else {
        Some(duplicate_fd(io::stdout())?)
    };
    let console_in =
        if let Some(console_in_path) = console_in_path {
            Some(File::open(console_in_path).with_context(|| {
                format!("Failed to open console input file {:?}", console_in_path)
            })?)
        } else {
            Some(duplicate_fd(io::stdin())?)
        };
    let log = if let Some(log_path) = log_path {
        Some(
            File::create(log_path)
                .with_context(|| format!("Failed to open log file {:?}", log_path))?,
        )
    } else {
        Some(duplicate_fd(io::stdout())?)
    };
    let callback = Box::new(Callback {});
    let vm = VmInstance::create(service, config, console_out, console_in, log, Some(callback))
        .context("Failed to create VM")?;
    vm.start().context("Failed to start VM")?;

    let debug_level = get_debug_level(config).unwrap_or(DebugLevel::NONE);

    println!(
        "Created {} from {} with CID {}, state is {}.",
        if debug_level == DebugLevel::FULL { "debuggable VM" } else { "VM" },
        payload_config,
        vm.cid(),
        state_to_str(vm.state()?)
    );

    // Wait until the VM or VirtualizationService dies. If we just returned immediately then the
    // IVirtualMachine Binder object would be dropped and the VM would be killed.
    let death_reason = vm.wait_for_death();
    println!("VM ended: {:?}", death_reason);
    Ok(())
}

fn parse_extra_apk_list(apk: &Path, config_path: &str) -> Result<Vec<PathBuf>, Error> {
    let mut archive = ZipArchive::new(File::open(apk)?)?;
    let config_file = archive.by_name(config_path)?;
    let config: VmPayloadConfig = serde_json::from_reader(config_file)?;
    Ok(config.extra_apks.into_iter().map(|x| x.path.into()).collect())
}

struct Callback {}

impl vmclient::VmCallback for Callback {
    fn on_payload_started(&self, _cid: i32) {
        eprintln!("payload started");
    }

    fn on_payload_ready(&self, _cid: i32) {
        eprintln!("payload is ready");
    }

    fn on_payload_finished(&self, _cid: i32, exit_code: i32) {
        eprintln!("payload finished with exit code {}", exit_code);
    }

    fn on_error(&self, _cid: i32, error_code: ErrorCode, message: &str) {
        eprintln!("VM encountered an error: code={:?}, message={}", error_code, message);
    }
}

/// Safely duplicate the file descriptor.
fn duplicate_fd<T: AsFd>(file: T) -> io::Result<File> {
    Ok(file.as_fd().try_clone_to_owned()?.into())
}
