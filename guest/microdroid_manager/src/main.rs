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

//! Microdroid Manager

mod cgroup_monitor;
mod dice;
mod encrypted_store_kek;
mod instance;
mod ioutil;
mod payload;
mod swap;
mod verify;
mod vm_internal_service;
mod vm_payload_service;
mod vm_secret;

use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::ErrorCode::ErrorCode;
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use android_system_virtualization_internal::aidl::android::system::virtualization::internal::IVmInternalService::{
    BnVmInternalService, VM_INTERNAL_SERVICE_SOCKET_NAME,
};
use android_system_virtualization_payload::aidl::android::system::virtualization::payload::IVmPayloadService::{
    BnVmPayloadService,
    VM_APK_CONTENTS_PATH,
    VM_PAYLOAD_SERVICE_SOCKET_NAME,
    ENCRYPTEDSTORE_MOUNTPOINT,
};
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IGuestAgent::{
    BnGuestAgent, IGuestAgent,
};

use crate::cgroup_monitor::start_cgroup_monitor;
use crate::dice::dice_derivation;
use crate::encrypted_store_kek::{decrypt_kek, encrypt_kek};
use crate::instance::{EncryptedStoreMode, InstanceDisk, MicrodroidData};
use crate::verify::verify_payload;
use crate::vm_internal_service::VmInternalService;
use crate::vm_payload_service::VmPayloadService;
use anyhow::{anyhow, bail, ensure, Context, Error, Result};
use binder::{self, BinderFeatures, Interface, IntoBinderResult, SpIBinder, Strong};
use dice_driver::DiceDriver;
use glob::glob;
use keystore2_crypto::ZVec;
use libc::{VMADDR_CID_HOST, VMADDR_PORT_ANY};
use log::{error, info, warn};
use microdroid_metadata::{Metadata, PayloadMetadata};
use microdroid_payload_config::{ApkConfig, OsConfig, Task, TaskType, VmPayloadConfig};
use nix::mount::{umount2, MntFlags};
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use payload::load_metadata;
#[cfg(vm_to_host_services)]
use rpc_servicemanager::register_rpc_servicemanager;
use rpcbinder::{RpcServer, RpcSession};
use rustutils::sockets::android_get_control_socket;
use rustutils::system_properties;
use rustutils::system_properties::PropertyWatcher;
use secretkeeper_comm::data_types::ID_SIZE;
use std::borrow::Cow::{Borrowed, Owned};
use std::env;
use std::ffi::CString;
use std::fs::{self, create_dir, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::raw::c_char;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::OwnedFd;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::ptr;
use std::str;
use std::sync::Arc;
use std::time::Duration;
use vm_secret::VmSecret;
use vsock::{VsockAddr, VsockListener, VsockStream, VMADDR_CID_ANY};

const WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const AVF_STRICT_BOOT: &str = "/proc/device-tree/chosen/avf,strict-boot";
const AVF_NEW_INSTANCE: &str = "/proc/device-tree/chosen/avf,new-instance";
const AVF_DEBUG_POLICY_RAMDUMP: &str = "/proc/device-tree/avf/guest/common/ramdump";
const DEBUG_MICRODROID_NO_VERIFIED_BOOT: &str =
    "/proc/device-tree/virtualization/guest/debug-microdroid,no-verified-boot";
const SECRETKEEPER_KEY: &str = "/proc/device-tree/avf/secretkeeper_public_key";
const INSTANCE_ID_PATH: &str = "/proc/device-tree/avf/untrusted/instance-id";
const DEFER_ROLLBACK_PROTECTION: &str = "/proc/device-tree/avf/untrusted/defer-rollback-protection";

const ENCRYPTEDSTORE_BIN: &str = "/system/bin/encryptedstore";
const ZIPFUSE_BIN: &str = "/system/bin/zipfuse";

const APEX_CONFIG_DONE_PROP: &str = "apex_config.done";
const DEBUGGABLE_PROP: &str = "ro.boot.microdroid.debuggable";

// SYNC WITH virtualizationservice/src/crosvm.rs
const FAILURE_SERIAL_DEVICE: &str = "/dev/ttyS1";

const ENCRYPTEDSTORE_BACKING_DEVICE: &str = "/dev/block/by-name/encryptedstore";
const ENCRYPTEDSTORE_KEYSIZE: usize = 32;

const DICE_CHAIN_FILE: &str = "/microdroid_resources/dice_chain.raw";

const ENCRYPTED_STORE_STATUS_PROP: &str = "microdroid_manager.encrypted_store.status";
const ENCRYPTED_STORE_SETUP_PROP: &str = "microdroid_manager.encrypted_store.setup";

#[derive(thiserror::Error, Debug)]
enum MicrodroidError {
    #[error("Cannot connect to virtualization service: {0}")]
    FailedToConnectToVirtualizationService(String),
    #[error("Payload has changed: {0}")]
    PayloadChanged(String),
    #[error("Payload verification has failed: {0}")]
    PayloadVerificationFailed(String),
    #[error("Payload config is invalid: {0}")]
    PayloadInvalidConfig(String),
}

fn translate_error(err: &Error) -> (ErrorCode, String) {
    if let Some(e) = err.downcast_ref::<MicrodroidError>() {
        match e {
            MicrodroidError::PayloadChanged(msg) => (ErrorCode::PAYLOAD_CHANGED, msg.to_string()),
            MicrodroidError::PayloadVerificationFailed(msg) => {
                (ErrorCode::PAYLOAD_VERIFICATION_FAILED, msg.to_string())
            }
            MicrodroidError::PayloadInvalidConfig(msg) => {
                (ErrorCode::PAYLOAD_INVALID_CONFIG, msg.to_string())
            }
            // Connection failure won't be reported to VS; return the default value
            MicrodroidError::FailedToConnectToVirtualizationService(msg) => {
                (ErrorCode::UNKNOWN, msg.to_string())
            }
        }
    } else {
        (ErrorCode::UNKNOWN, err.to_string())
    }
}

fn write_death_reason_to_serial(err: &Error) -> Result<()> {
    let death_reason = if let Some(e) = err.downcast_ref::<MicrodroidError>() {
        Borrowed(match e {
            MicrodroidError::FailedToConnectToVirtualizationService(_) => {
                "MICRODROID_FAILED_TO_CONNECT_TO_VIRTUALIZATION_SERVICE"
            }
            MicrodroidError::PayloadChanged(_) => "MICRODROID_PAYLOAD_HAS_CHANGED",
            MicrodroidError::PayloadVerificationFailed(_) => {
                "MICRODROID_PAYLOAD_VERIFICATION_FAILED"
            }
            MicrodroidError::PayloadInvalidConfig(_) => "MICRODROID_INVALID_PAYLOAD_CONFIG",
        })
    } else {
        // Send context information back after a separator, to ease diagnosis.
        // These errors occur before the payload runs, so this should not leak sensitive
        // information.
        Owned(format!("MICRODROID_UNKNOWN_RUNTIME_ERROR|{:?}", err))
    };

    let mut serial_file = OpenOptions::new().read(false).write(true).open(FAILURE_SERIAL_DEVICE)?;
    serial_file.write_all(death_reason.as_bytes()).context("serial device write_all failed")?;
    // Block until the serial port trasmits all the data to the host.
    nix::sys::termios::tcdrain(&serial_file).context("tcdrain failed")?;

    Ok(())
}

#[derive(Debug)]
struct SeContext(*mut ::std::os::raw::c_char);
impl SeContext {
    fn new(file: &File) -> Result<Self> {
        let fd = file.as_raw_fd();
        let mut con: *mut c_char = ptr::null_mut();
        // SAFETY: the returned pointer `con` is wrapped in SeContext which is freed with
        // `freecon` when it is dropped.
        match unsafe { selinux_bindgen::fgetfilecon(fd, &mut con) } {
            1.. => {
                if !con.is_null() {
                    Ok(Self(con))
                } else {
                    Err(anyhow!("fgetfilecon returned a NULL context"))
                }
            }
            _ => Err(anyhow!(std::io::Error::last_os_error())).context("fgetfilecon failed"),
        }
    }
}

impl Drop for SeContext {
    fn drop(&mut self) {
        // SAFETY: SeContext is created only with a pointer that is set by libselinux and
        // has to be freed with freecon.
        unsafe { selinux_bindgen::freecon(self.0) };
    }
}

impl std::fmt::Display for SeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            // SAFETY: the non-owned C string pointed by `p` is guaranteed to be valid (non-null
            // and shorter than i32::MAX). It is freed when SeContext is dropped.
            unsafe { std::ffi::CStr::from_ptr(self.0) }.to_str().unwrap_or("Invalid context")
        )
    }
}

fn debug_logs_encryptedstore() -> Result<()> {
    let file = File::open(ENCRYPTEDSTORE_MOUNTPOINT)?;
    // TODO: Ideally log this error instead of propagating it out of a debug function
    let file_context = SeContext::new(&file)?;
    log::info!(
        "encryptedstore permission mode {:o}, file context {}",
        file.metadata()?.permissions().mode(),
        file_context
    );
    Ok(())
}

/// The (host allocated) instance_id can be found at node /avf/untrusted/ in the device tree.
fn get_instance_id() -> Result<Option<[u8; ID_SIZE]>> {
    let path = Path::new(INSTANCE_ID_PATH);
    let instance_id = if path.exists() {
        Some(
            fs::read(path)?
                .try_into()
                .map_err(|x: Vec<_>| anyhow!("Expected {ID_SIZE} bytes, found {:?}", x.len()))?,
        )
    } else {
        // TODO(b/325094712): x86 support for Device tree in nested guest is limited/broken/
        // untested. So instance_id will not be present in cuttlefish.
        None
    };
    Ok(instance_id)
}

fn should_defer_rollback_protection() -> bool {
    Path::new(DEFER_ROLLBACK_PROTECTION).exists()
}

/// Configure the balloon device to not retry when it fails to inflate. Context: b/407629285
fn set_bail_on_out_of_puff() -> Result<()> {
    // The sysfs path will look like the following, but `N` varies.
    //
    //     /sys/bus/virtio/drivers/virtio_balloon/virtioN/bail_on_out_of_puff"
    for entry in std::fs::read_dir("/sys/bus/virtio/drivers/virtio_balloon")? {
        let entry = entry?;
        match entry.file_name().to_str() {
            Some(name) if name.starts_with("virtio") => {}
            _ => continue,
        }
        let option_path = entry.path().join("bail_on_out_of_puff");
        if !option_path.exists() {
            continue;
        }
        std::fs::write(option_path, "Y")?;
        return Ok(());
    }
    bail!("didn't find bail_on_out_of_puff sysfs entry")
}

/// Ignore SIGTERM so that we wait for the termination of microdroid_launcher.
/// The termination of microdroid_launcher is also via SIGTERM sent to the process group where
/// both processes belong to.
fn setup_ignore_sigterm() -> Result<()> {
    let sa = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());

    // SAFETY: we are not doing any action in the handler
    unsafe { sigaction(Signal::SIGTERM, &sa) }
        .context("Failed to set sigaction for SIGTERM")
        .map(|_| ())
}

fn main() -> Result<()> {
    // SAFETY: This is very early in the process. Nobody has taken ownership of the inherited FDs
    // yet.
    unsafe { rustutils::inherited_fd::init_once()? };

    // If debuggable, print full backtrace to console log with stdio_to_kmsg
    if is_debuggable() {
        env::set_var("RUST_BACKTRACE", "full");
    }

    scopeguard::defer! {
        info!("Shutting down...");
        if let Err(e) = system_properties::write("sys.powerctl", "shutdown") {
            error!("failed to shutdown {:?}", e);
        }
    }

    try_main().map_err(|e| {
        error!("Failed with {:?}.", e);
        if let Err(e) = write_death_reason_to_serial(&e) {
            error!("Failed to write death reason {:?}", e);
        }
        e
    })
}

fn try_main() -> Result<()> {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("microdroid_manager")
            .with_max_level(log::LevelFilter::Info),
    );
    info!("started.");

    load_crashkernel_if_supported().context("Failed to load crashkernel")?;

    swap::init_swap().context("Failed to initialize swap")?;
    info!("swap enabled.");

    if let Err(e) = set_bail_on_out_of_puff() {
        warn!("failed to set bail_on_out_of_puff: {e:#}");
    }

    let service = get_vms_rpc_binder()
        .context("cannot connect to VirtualMachineService")
        .map_err(|e| MicrodroidError::FailedToConnectToVirtualizationService(e.to_string()))?;

    let guest_agent = GuestAgent::new_binder();
    service.registerGuestAgent(&guest_agent)?;

    #[cfg(vm_to_host_services)]
    register_rpc_servicemanager(
        service
            .getHostRpcProvider()
            .context("failed to set up the host RPC provider from the host")?,
    )?;

    let vm_internal_service_fd = android_get_control_socket(VM_INTERNAL_SERVICE_SOCKET_NAME)?;
    let vm_payload_service_fd = android_get_control_socket(VM_PAYLOAD_SERVICE_SOCKET_NAME)?;

    match try_run_payload(&service, vm_internal_service_fd, vm_payload_service_fd) {
        Ok(code) => {
            match code {
                0 => info!("task successfully finished"),
                v => error!("task exited with exit code: {}", v),
            };
            if let Err(e) = post_payload_work() {
                error!(
                    "Failed to run post payload work. It is possible that certain tasks
                    like syncing encrypted store might be incomplete. Error: {:?}",
                    e
                );
            };

            info!("notifying payload finished");
            service.notifyPayloadFinished(code)?;
            Ok(())
        }
        Err(err) => {
            warn!("payload finished erroneously: {:?}", err);
            let (error_code, message) = translate_error(&err);
            service.notifyError(error_code, &message)?;
            Err(err)
        }
    }
}

fn verify_payload_with_instance_img(
    metadata: &Metadata,
    dice: &DiceDriver,
    state: &mut VmInstanceState,
) -> Result<MicrodroidData> {
    let mut instance = InstanceDisk::new().context("Failed to load instance.img")?;
    let saved_data = instance.read_microdroid_data(dice).context("Failed to read identity data")?;

    if is_strict_boot() {
        // Provisioning must happen on the first boot and never again.
        if Path::new(AVF_NEW_INSTANCE).exists() {
            ensure!(
                saved_data.is_none(),
                MicrodroidError::PayloadInvalidConfig(
                    "Found instance data on first boot.".to_string()
                )
            );
        } else {
            ensure!(
                saved_data.is_some(),
                MicrodroidError::PayloadInvalidConfig("Instance data not found.".to_string())
            );
        };
    }

    // Verify the payload before using it.
    let extracted_data = verify_payload(metadata, saved_data.as_ref())
        .context("Payload verification failed")
        .map_err(|e| MicrodroidError::PayloadVerificationFailed(format!("{:?}", e)))?;

    // In case identity is ignored (by debug policy), we should reuse existing payload data, even
    // when the payload is changed. This is to keep the derived secret same as before.
    let instance_data = if let Some(saved_data) = saved_data {
        if !is_verified_boot() {
            if saved_data != extracted_data {
                info!("Detected an update of the payload, but continue (regarding debug policy)")
            }
        } else {
            ensure!(
                saved_data == extracted_data,
                MicrodroidError::PayloadChanged(String::from(
                    "Detected an update of the payload which isn't supported yet."
                ))
            );
            info!("Saved data is verified.");
        }
        *state = VmInstanceState::PreviouslySeen;
        saved_data
    } else {
        info!("Saving verified data.");
        instance
            .write_microdroid_data(&extracted_data, dice)
            .context("Failed to write identity data")?;
        *state = VmInstanceState::NewlyCreated;
        extracted_data
    };
    Ok(instance_data)
}

// The VM instance run can be
// 1. Either Newly created - which can happen if this is really a new VM instance (or a malicious
//    Android has deleted relevant secrets)
// 2. Or Re-run from an already seen VM instance.
#[derive(PartialEq, Eq)]
enum VmInstanceState {
    Unknown,
    NewlyCreated,
    PreviouslySeen,
}

struct CgroupConfig {
    name: &'static str,
    // Limits how much memory the cgroup (generally just the payload process) can consume before
    // reclaim starts running on that cgroup.
    memory_high_mib: u64,
}

fn try_run_payload(
    service: &Strong<dyn IVirtualMachineService>,
    vm_internal_service_fd: OwnedFd,
    vm_payload_service_fd: OwnedFd,
) -> Result<i32> {
    let metadata = load_metadata().context("Failed to load payload metadata")?;
    let dice = if Path::new(DICE_CHAIN_FILE).exists() {
        DiceDriver::from_file(Path::new(DICE_CHAIN_FILE))
            .context("Failed to load DICE from file")?
    } else {
        DiceDriver::new(Path::new("/dev/open-dice0"), is_strict_boot())
            .context("Failed to load DICE from driver")?
    };

    let mut state = VmInstanceState::Unknown;
    // Microdroid skips checking payload against instance image iff the device supports
    // secretkeeper. In that case Microdroid use VmSecret::V2, which provides instance state
    // and protection against rollback of boot images and packages.
    let instance_data = if should_defer_rollback_protection() {
        verify_payload(&metadata, None)?
    } else {
        verify_payload_with_instance_img(&metadata, &dice, &mut state)?
    };

    // TODO(b/426584173): Add an API for configuring cgroups. For now we hardcode a config for
    // appsearch's VM, which we detect indirectly via the rollback_index field.
    let cgroup_config = if instance_data.apk_data.rollback_index.is_some() {
        Some(CgroupConfig { name: "microdroid_launcher", memory_high_mib: 30 })
    } else {
        None
    };

    if let Some(cgroup_config) = cgroup_config.as_ref() {
        // We create and configure the cgroup now, then the child process adds itself to the group
        // before `exec`ing the payload binary (see `exec_task` code).
        let cgroup_dir = std::path::Path::new("/sys/fs/cgroup").join(cgroup_config.name);
        std::fs::create_dir(&cgroup_dir).context("failed to create cgroup dir")?;
        std::fs::write(
            cgroup_dir.join("memory.high"),
            format!("{}M", cgroup_config.memory_high_mib),
        )
        .context("failed to set cgroup memory.high")?;

        // Spawn thread to monitor the cgroup's behavior.
        // TODO: (khei@)
        // Send cgroup kill signal and join cgroup thread for graceful shutdown
        let (_cgroup_thread, _cgroup_kill) = start_cgroup_monitor(cgroup_config.name)?;
    }

    let payload_metadata = metadata.payload.ok_or_else(|| {
        MicrodroidError::PayloadInvalidConfig("No payload config in metadata".to_string())
    })?;

    // To minimize the exposure to untrusted data, derive dice profile as soon as possible.
    info!("DICE derivation for payload");
    let dice_artifacts = dice_derivation(dice, &instance_data, &payload_metadata)?;
    let vm_secret = Arc::new(
        VmSecret::new(dice_artifacts, service, &mut state)
            .context("Failed to create VM secrets")?,
    );

    let is_new_instance = match state {
        VmInstanceState::NewlyCreated => true,
        VmInstanceState::PreviouslySeen => false,
        VmInstanceState::Unknown => {
            bail!("Vm instance state is still unknown, this should not have happened");
        }
    };

    if cfg!(dice_changes) {
        // Now that the DICE derivation is done, it's ok to allow payload code to run.

        // Start apexd to activate APEXes. This may allow code within them to run.
        system_properties::write("ctl.start", "apexd-vm")?;

        // Unmounting /microdroid_resources is a defence-in-depth effort to ensure that payload
        // can't get hold of dice chain stored there.
        umount2("/microdroid_resources", MntFlags::MNT_DETACH)?;
    }

    let mut zipfuse = Zipfuse::default();

    // Before reading a file from the APK, start zipfuse
    zipfuse.mount(
        "fscontext=u:object_r:zipfusefs:s0,context=u:object_r:system_file:s0",
        Path::new(verify::DM_MOUNTED_APK_PATH),
        Path::new(VM_APK_CONTENTS_PATH),
        "microdroid_manager.apk.mounted".to_owned(),
    )?;

    // Restricted APIs are only allowed to be used by platform or test components. Infer this from
    // the use of a VM config file since those can only be used by platform and test components.
    let allow_restricted_apis = match payload_metadata {
        PayloadMetadata::ConfigPath(_) => true,
        PayloadMetadata::Config(_) => false,
        _ => false, // default is false for safety
    };

    let config = load_config(payload_metadata).context("Failed to load payload metadata")?;

    let task = config
        .task
        .as_ref()
        .ok_or_else(|| MicrodroidError::PayloadInvalidConfig("No task in VM config".to_string()))?;

    ensure!(
        config.extra_apks.len() == instance_data.extra_apks_data.len(),
        "config expects {} extra apks, but found {}",
        config.extra_apks.len(),
        instance_data.extra_apks_data.len()
    );
    mount_extra_apks(&config, &mut zipfuse)?;

    // Wait until apex config is done. (e.g. linker configuration for apexes)
    wait_for_property_true(APEX_CONFIG_DONE_PROP).context("Failed waiting for apex config done")?;

    let std_redirect = if is_debuggable() {
        // If the VM is debuggable, let stdout/stderr go outside via /dev/kmsg to ease the debugging
        Arc::new(Some(rustutils::inherited_fd::take_fd_ownership(
            env::var("ANDROID_FILE__dev_kmsg").unwrap().parse::<i32>().unwrap(),
        )?))
    } else {
        Arc::new(None)
    };

    let vm_internal_binder = BnVmInternalService::new_binder(
        VmInternalService::new(service.clone()),
        BinderFeatures::default(),
    );

    spawn_binder_rpc_server(
        vm_internal_binder.as_binder(),
        vm_internal_service_fd,
        VM_INTERNAL_SERVICE_SOCKET_NAME,
    )?;

    // Run encryptedstore binary to prepare the storage
    // Postpone initialization until apex mount completes to ensure e2fsck and resize2fs binaries
    // are accessible.
    let encryptedstore_child = if Path::new(ENCRYPTEDSTORE_BACKING_DEVICE).exists() {
        let std_redirect_for_enc_store = std_redirect.clone();
        if config.delay_encrypted_store_setup {
            let service_clone = service.clone();
            let vm_secret_for_enc_store = vm_secret.clone();
            let encrypted_store_mode = instance_data.apk_data.encrypted_store_mode;
            info!("Delaying preparation of encryptedstore as requested ...");
            std::thread::spawn(move || {
                // Should we violently crash here? Or should we just log the error and let payload
                // decide what to do?
                if let Err(e) = delayed_prepare_encryptedstore(
                    encrypted_store_mode,
                    service_clone,
                    vm_secret_for_enc_store,
                    std_redirect_for_enc_store,
                ) {
                    error!("delayed prepare encrypted store failed: {:#?}", e);
                }
            });
            None
        } else {
            info!("Preparing encryptedstore ...");
            let mut key = ZVec::new(ENCRYPTEDSTORE_KEYSIZE)?;
            vm_secret.derive_encryptedstore_key(&mut key).context("derive encrypted store key")?;
            Some(
                prepare_encryptedstore(&key, &std_redirect_for_enc_store)
                    .context("encryptedstore run")?,
            )
        }
    } else {
        None
    };

    let vm_payload_binder = BnVmPayloadService::new_binder(
        VmPayloadService::new(
            allow_restricted_apis,
            service.clone(),
            vm_secret.clone(),
            is_new_instance,
        ),
        BinderFeatures::default(),
    );

    spawn_binder_rpc_server(
        vm_payload_binder.as_binder(),
        vm_payload_service_fd,
        VM_PAYLOAD_SERVICE_SOCKET_NAME,
    )?;

    // Set export_tombstones if enabled
    if should_export_tombstones(&config) {
        // This property is read by tombstone_handler.
        system_properties::write("microdroid_manager.export_tombstones.enabled", "1")
            .context("set microdroid_manager.export_tombstones.enabled")?;
    }

    // Trigger init post-fs-data. This will start authfs if we wask it to.
    if config.enable_authfs {
        system_properties::write("microdroid_manager.authfs.enabled", "1")
            .context("failed to write microdroid_manager.authfs.enabled")?;
    }
    system_properties::write("microdroid_manager.config_done", "1")
        .context("failed to write microdroid_manager.config_done")?;

    // Wait until zipfuse has mounted the APKs so we can access the payload
    zipfuse.wait_until_done()?;

    // Wait for encryptedstore to finish mounting the storage (if enabled) before setting
    // microdroid_manager.init_done. Reason is init stops uneventd after that.
    // Encryptedstore, however requires ueventd
    if let Some(mut child) = encryptedstore_child {
        let exitcode = child.wait().context("Wait for encryptedstore child")?;
        ensure!(exitcode.success(), "Unable to prepare encrypted storage. Exitcode={}", exitcode);
        // Wait until init performs restorecon on /mnt/encryptedstore
        wait_for_property(ENCRYPTED_STORE_STATUS_PROP, "ready")
            .context("Wait for {ENCRYPTED_STORE_STATUS_PROP}")?;
        debug_logs_encryptedstore()?;
    }

    // Wait for init to have finished booting.
    wait_for_property_true("dev.bootcomplete").context("failed waiting for dev.bootcomplete")?;

    // And then tell it we're done so unnecessary services can be shut down.
    // Right now the only service we stop is ueventd. However, in case payload request to delay
    // setup of the encrypted store, we should keep the ueventd around.
    if !config.delay_encrypted_store_setup {
        system_properties::write("microdroid_manager.init_done", "1")
            .context("set microdroid_manager.init_done")?;
    }

    info!("boot completed, time to run payload");
    let mut payload_process = exec_task(task, cgroup_config.as_ref(), service, &std_redirect)
        .context("Failed to run payload")?;
    setup_ignore_sigterm()?;

    let exit_status = payload_process.wait()?;
    match exit_status.code() {
        Some(exit_code) => Ok(exit_code),
        None => match exit_status.signal() {
            Some(val) if val == Signal::SIGTERM as i32 => {
                info!("payload exited with SIGTERM");
                Ok(0)
            }
            Some(signal) => Err(anyhow!(
                "Payload exited due to signal: {} ({})",
                signal,
                Signal::try_from(signal).map_or("unknown", |s| s.as_str())
            )),
            None => Err(anyhow!("Payload has neither exit code nor signal")),
        },
    }
}

fn spawn_binder_rpc_server(binder: SpIBinder, fd: OwnedFd, name: &str) -> Result<()> {
    let server = RpcServer::new_bound_socket(binder, fd)?;
    info!("The RPC server '{name}' is running.");

    // Move server reference into a background thread and run it forever.
    std::thread::spawn(move || {
        server.join();
    });

    Ok(())
}

fn post_payload_work() -> Result<()> {
    // Sync the encrypted storage filesystem (flushes the filesystem caches).
    if Path::new(ENCRYPTEDSTORE_BACKING_DEVICE).exists() {
        let mountpoint = CString::new(ENCRYPTEDSTORE_MOUNTPOINT).unwrap();

        // SAFETY: `mountpoint` is a valid C string. `syncfs` and `close` are safe for any parameter
        // values.
        let ret = unsafe {
            let dirfd = libc::open(
                mountpoint.as_ptr(),
                libc::O_DIRECTORY | libc::O_RDONLY | libc::O_CLOEXEC,
            );
            ensure!(dirfd >= 0, "Unable to open {:?}", mountpoint);
            let ret = libc::syncfs(dirfd);
            libc::close(dirfd);
            ret
        };
        if ret != 0 {
            error!("failed to sync encrypted storage.");
            return Err(anyhow!(std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

fn mount_extra_apks(config: &VmPayloadConfig, zipfuse: &mut Zipfuse) -> Result<()> {
    // For now, only the number of apks is important, as the mount point and dm-verity name is fixed
    for i in 0..config.extra_apks.len() {
        let mount_dir = format!("/mnt/extra-apk/{i}");
        create_dir(Path::new(&mount_dir)).context("Failed to create mount dir for extra apks")?;

        // These run asynchronously in parallel - we wait later for them to complete.
        zipfuse.mount(
            "fscontext=u:object_r:zipfusefs:s0,context=u:object_r:extra_apk_file:s0",
            Path::new(&format!("/dev/block/mapper/extra-apk-{i}")),
            Path::new(&mount_dir),
            format!("microdroid_manager.extra_apk.mounted.{i}"),
        )?;
    }

    Ok(())
}

fn get_vms_rpc_binder() -> Result<Strong<dyn IVirtualMachineService>> {
    // The host is running a VirtualMachineService for this VM on a port equal
    // to the CID of this VM.
    let port = vsock::get_local_cid().context("Could not determine local CID")?;
    let session = RpcSession::new();
    session.set_max_incoming_threads(1);
    session
        .setup_vsock_client(VMADDR_CID_HOST, port)
        .context("Could not connect to IVirtualMachineService")
}

fn is_strict_boot() -> bool {
    Path::new(AVF_STRICT_BOOT).exists()
}

fn is_verified_boot() -> bool {
    !Path::new(DEBUG_MICRODROID_NO_VERIFIED_BOOT).exists()
}

/// Returns true iff the VM is successfully identified as being debuggable.
fn is_debuggable() -> bool {
    system_properties::read_bool(DEBUGGABLE_PROP, false).unwrap_or(false)
}

fn should_export_tombstones(config: &VmPayloadConfig) -> bool {
    match config.export_tombstones {
        Some(b) => b,
        None => is_debuggable(),
    }
}

/// Get debug policy value in bool. It's true iff the value was successfully read and explicitly
/// set to <1>.
fn get_debug_policy_bool(path: &'static str) -> bool {
    let mut log: [u8; 4] = Default::default();
    if let Err(e) = File::open(path).map(|mut f| f.read_exact(&mut log)) {
        info!("Assume debug policy is disabled because of a failed read ({e:?})");
        false
    } else {
        u32::from_be_bytes(log) == 1
    }
}

#[derive(Default)]
struct Zipfuse {
    ready_properties: Vec<String>,
}

impl Zipfuse {
    fn mount(
        &mut self,
        option: &str,
        zip_path: &Path,
        mount_dir: &Path,
        ready_prop: String,
    ) -> Result<Child> {
        let mut cmd = Command::new(ZIPFUSE_BIN);
        // Let root own the files in APK, so we can access them, but set the group to
        // allow all payloads to have access too.
        let (uid, gid) = (microdroid_uids::ROOT_UID, microdroid_uids::MICRODROID_PAYLOAD_GID);

        cmd.args(["-p", &ready_prop, "-o", option]);
        cmd.args(["-u", &uid.to_string()]);
        cmd.args(["-g", &gid.to_string()]);
        cmd.arg(zip_path).arg(mount_dir);
        self.ready_properties.push(ready_prop);
        cmd.spawn().with_context(|| format!("Failed to run zipfuse for {mount_dir:?}"))
    }

    fn wait_until_done(self) -> Result<()> {
        // We check the last-started check first in the hope that by the time it is done
        // all or most of the others will also be done, minimising the number of times we
        // block on a property.
        for property in self.ready_properties.into_iter().rev() {
            wait_for_property_true(&property)
                .with_context(|| format!("Failed waiting for {property}"))?;
        }
        Ok(())
    }
}

fn wait_for_property_true(property_name: &str) -> Result<()> {
    let mut prop = PropertyWatcher::new(property_name)?;
    loop {
        prop.wait(None)?;
        if system_properties::read_bool(property_name, false)? {
            break;
        }
    }
    Ok(())
}

fn wait_for_property(property_name: &str, expected_value: &str) -> Result<()> {
    let mut prop = PropertyWatcher::new(property_name)?;
    loop {
        prop.wait(None)?;
        if let Some(value) = system_properties::read(property_name)? {
            if value == expected_value {
                break;
            }
        }
    }
    Ok(())
}

fn load_config(payload_metadata: PayloadMetadata) -> Result<VmPayloadConfig> {
    match payload_metadata {
        PayloadMetadata::ConfigPath(path) => {
            let path = Path::new(&path);
            info!("loading config from {:?}...", path);
            let file = ioutil::wait_for_file(path, WAIT_TIMEOUT)
                .with_context(|| format!("Failed to read {:?}", path))?;
            Ok(serde_json::from_reader(file)?)
        }
        PayloadMetadata::Config(payload_config) => {
            let task = Task {
                type_: TaskType::MicrodroidLauncher,
                command: payload_config.payload_binary_name,
            };
            // We don't care about the paths, only the number of extra APKs really matters.
            let extra_apks = (0..payload_config.extra_apk_count)
                .map(|i| ApkConfig { path: format!("extra-apk-{i}") })
                .collect();
            Ok(VmPayloadConfig {
                os: OsConfig { name: "microdroid".to_owned() },
                task: Some(task),
                apexes: vec![],
                extra_apks,
                prefer_staged: false,
                export_tombstones: None,
                enable_authfs: false,
                hugepages: false,
                delay_encrypted_store_setup: payload_config.delay_encrypted_store_setup,
            })
        }
        _ => bail!("Failed to match config against a config type."),
    }
}

/// Loads the crashkernel into memory using kexec if debuggable or debug policy says so.
/// The VM should be loaded with `crashkernel=' parameter in the cmdline to allocate memory
/// for crashkernel.
fn load_crashkernel_if_supported() -> Result<()> {
    let supported = std::fs::read_to_string("/proc/cmdline")?.contains(" crashkernel=");
    info!("ramdump supported: {}", supported);

    if !supported {
        return Ok(());
    }

    let debuggable = is_debuggable();
    let ramdump = get_debug_policy_bool(AVF_DEBUG_POLICY_RAMDUMP);
    let requested = debuggable | ramdump;

    if requested {
        let status = Command::new("/system/bin/kexec_load").status()?;
        if !status.success() {
            return Err(anyhow!("Failed to load crashkernel: {status}"));
        }
        info!("ramdump is loaded: debuggable={debuggable}, ramdump={ramdump}");
    }
    Ok(())
}

/// Executes the given task.
fn exec_task(
    task: &Task,
    cgroup_config: Option<&CgroupConfig>,
    service: &Strong<dyn IVirtualMachineService>,
    std_redirect: &Option<OwnedFd>,
) -> Result<Child> {
    info!("executing main task {:?}...", task);
    let (mut command, uid_gid) = match task.type_ {
        TaskType::Executable => {
            // TODO(b/297501338): Figure out how to handle non-root for system payloads.
            (Command::new(&task.command), None)
        }
        TaskType::MicrodroidLauncher => {
            let mut command = Command::new("/system/bin/microdroid_launcher");
            command.arg(find_library_path(&task.command)?);
            (
                command,
                Some((
                    microdroid_uids::MICRODROID_PAYLOAD_UID,
                    microdroid_uids::MICRODROID_PAYLOAD_GID,
                )),
            )
        }
    };

    let cgroup_path = cgroup_config.map(|c| format!("/sys/fs/cgroup/{}/cgroup.procs", c.name));

    // SAFETY: We are not accessing any resource of the parent process. This means we can't make any
    // log calls inside the closure.
    unsafe {
        command.pre_exec(move || {
            // Move the payload process into a cgroup.
            //
            // We can't ignore errors here because we rely on the cgroup to restrict the process'
            // resource usage. We can't log, so just abort on error and microdroid_manager will see
            // something is wrong.
            if let Some(cgroup_path) = &cgroup_path {
                let mut buffer = itoa::Buffer::new();
                let pid_str = buffer.format(std::process::id()).as_bytes();
                std::fs::write(cgroup_path, pid_str).unwrap_or_else(|_| std::process::abort());
            }
            // Set UID and GID. Has to happen after changing the cgroup. Can't use rust's
            // `Command::uid/gid` because they are applied before the `pre_exec` hook.
            if let Some((uid, gid)) = uid_gid {
                nix::unistd::setgid(nix::unistd::Gid::from_raw(gid))
                    .unwrap_or_else(|_| std::process::abort());
                nix::unistd::setuid(nix::unistd::Uid::from_raw(uid))
                    .unwrap_or_else(|_| std::process::abort());
            }
            // It is OK to continue with payload execution even if the calls below fail, since
            // whether process can use a capability is controlled by the SELinux. Dropping the
            // capabilities here is just another defense-in-depth layer.
            let _ = cap::drop_inheritable_caps();
            let _ = cap::drop_bounding_set();
            Ok(())
        });
    }

    // Never accept input from outside
    command.stdin(Stdio::null());

    let (stdout, stderr) = if let Some(fd) = std_redirect {
        (Stdio::from(fd.try_clone()?), Stdio::from(fd.try_clone()?))
    } else {
        (Stdio::null(), Stdio::null())
    };
    command.stdout(stdout);
    command.stderr(stderr);

    info!("notifying payload started");
    service.notifyPayloadStarted()?;

    let payload_process = command.spawn().context("failed to spawn payload process")?;
    info!("payload pid = {:?}", payload_process.id());

    // SAFETY: setpriority doesn't take any pointers
    unsafe {
        let ret = libc::setpriority(libc::PRIO_PROCESS, payload_process.id(), -20);
        if ret != 0 {
            error!(
                "failed to adjust priority of the payload: {:#?}",
                std::io::Error::last_os_error()
            );
        }
    }
    Ok(payload_process)
}

fn find_library_path(name: &str) -> Result<String> {
    let mut watcher = PropertyWatcher::new("ro.product.cpu.abilist")?;
    let value = watcher.read(|_name, value| Ok(value.trim().to_string()))?;
    let abi = value.split(',').next().ok_or_else(|| anyhow!("no abilist"))?;

    let paths = [
        format!("{}/lib/{}/{}", VM_APK_CONTENTS_PATH, abi, name),
        // TODO(b/372535544): standardize
        "/apex/com.android.appsearch/lib64/libicing_anywhere.so".to_string(),
    ];

    for path_str in &paths {
        let path = PathBuf::from(path_str);
        if let Ok(metadata) = fs::metadata(&path) {
            if metadata.is_file() {
                return Ok(path_str.to_string());
            }
        }
    }

    bail!("None of the specified paths are valid files: {:?}", paths);
}

fn prepare_encryptedstore(key: &[u8], std_redirect: &Option<OwnedFd>) -> Result<Child> {
    let (stdout, stderr) = if let Some(fd) = std_redirect {
        (Stdio::from(fd.try_clone()?), Stdio::from(fd.try_clone()?))
    } else {
        (Stdio::null(), Stdio::null())
    };
    let mut cmd = Command::new(ENCRYPTEDSTORE_BIN);
    cmd.arg("--blkdevice")
        .arg(ENCRYPTEDSTORE_BACKING_DEVICE)
        .arg("--key")
        .arg(hex::encode(key))
        .args(["--mountpoint", ENCRYPTEDSTORE_MOUNTPOINT])
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .context("encryptedstore failed")
}

/// Implementation of `IGuestAgent`
#[derive(Debug, Default)]
struct GuestAgent {}

impl Interface for GuestAgent {}

impl IGuestAgent for GuestAgent {
    fn startDumpVsockServer(&self, args: &[String]) -> binder::Result<i32> {
        info!("Default dump handler with args: {args:?}");
        start_dump_service().or_service_specific_exception(-1)
    }

    fn shutdownAsync(&self) -> binder::Result<()> {
        info!("Shutdown requested.");
        if let Err(e) = system_properties::write("sys.powerctl", "shutdown") {
            error!("failed to shutdown {:?}", e);
        }
        Ok(())
    }

    fn trimAsync(&self) -> binder::Result<()> {
        if let Err(e) = system_properties::write("pageout_bomb.go", "1") {
            error!("failed to set pageout_bomb.go: {:?}", e);
        }
        Ok(())
    }
}

impl GuestAgent {
    fn new_binder() -> Strong<dyn IGuestAgent> {
        BnGuestAgent::new_binder(GuestAgent {}, BinderFeatures::default())
    }
}

fn delayed_prepare_encryptedstore(
    encrypted_store_mode: EncryptedStoreMode,
    service: Strong<dyn IVirtualMachineService>,
    vm_secret: Arc<VmSecret>,
    std_redirect: Arc<Option<OwnedFd>>,
) -> Result<()> {
    info!("waiting for {ENCRYPTED_STORE_SETUP_PROP} to set up encrypted store");
    wait_for_property_true(ENCRYPTED_STORE_SETUP_PROP)
        .context("failed waiting for {ENCRYPTED_STORE_SETUP_PROP}")?;
    info!("{ENCRYPTED_STORE_SETUP_PROP} is true. Preparing encryptedstore ...");

    let mut key = ZVec::new(ENCRYPTEDSTORE_KEYSIZE)?;
    match encrypted_store_mode {
        EncryptedStoreMode::KEKsStoredOnHost => {
            get_encrypted_store_key(&service, &vm_secret, &mut key)
                .context("get encrypted store key")?;
        }
        EncryptedStoreMode::DefaultKey => {
            vm_secret.derive_encryptedstore_key(&mut key).context("derive encrypted store key")?;
        }
    }
    prepare_encryptedstore(&key, &std_redirect)?
        .wait()
        .context("failed waiting for encryptedstore binary to finish")?;

    wait_for_property(ENCRYPTED_STORE_STATUS_PROP, "ready")
        .context("wait for {ENCRYPTED_STORE_STATUS_PROP}")?;

    // Now we can tell ueventd to stop.
    system_properties::write("microdroid_manager.init_done", "1")
        .context("set microdroid_manager.init_done")
}

fn get_encrypted_store_key(
    service: &Strong<dyn IVirtualMachineService>,
    vm_secret: &VmSecret,
    key: &mut [u8],
) -> Result<()> {
    let kek_wrapper = service.getEncryptedStoreKEK().context("failed to get KEK")?;
    let kek_wrapper = if let Some(kek_wrapper) = kek_wrapper {
        kek_wrapper
    } else {
        bail!("expected encrypted store KEK from host but got nothing");
    };

    // This key is used to encrypt the key used for encrypted store setup.
    let mut encryption_key = ZVec::new(ENCRYPTEDSTORE_KEYSIZE)?;
    vm_secret
        .derive_encryptedstore_key_encryption_key(&mut encryption_key)
        .context("failed to derive encryptedstore_key encryption key")?;
    let kek = kek_wrapper.getKEK().context("failed to get KEK")?;
    if let Some(kek) = kek {
        let decrypted_key = decrypt_kek(&kek, &encryption_key).context("failed to decrypt KEK")?;
        key.copy_from_slice(&decrypted_key);
    } else {
        vm_secret.derive_random_key(key).context("derive random key")?;
        let encrypted_kek = encrypt_kek(key, &encryption_key).context("failed to encrypt KEK")?;
        kek_wrapper.onKEKCreated(&encrypted_kek).context("failed to send KEK to host")?;
    }

    Ok(())
}

fn start_dump_service() -> Result<i32> {
    let bind_addr = VsockAddr::new(VMADDR_CID_ANY, VMADDR_PORT_ANY);
    let listener = VsockListener::bind(&bind_addr).context("Can't bind vsock")?;
    let local_addr = listener.local_addr().context("Can't get local addr")?;

    std::thread::spawn(move || {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) => {
                error!("failed to accept on dump service: {e:?}");
                return;
            }
        };

        if let Err(e) = handle_dump_to_client(stream) {
            error!("failed to dump: {e:?}");
        }
    });

    Ok(local_addr.port() as i32)
}

fn read_and_write_file(stream: &mut VsockStream, file_path: &Path) -> Result<()> {
    let path_str = file_path.display().to_string();
    let header = format!("---- {} begin ----\n", &path_str);
    let footer = format!("\n---- {} end ----\n", &path_str);
    stream.write_all(header.as_bytes())?; // Write the file path header

    match File::open(file_path) {
        Ok(f) => {
            let mut reader = BufReader::new(f);

            if file_path.ends_with("smaps_rollup") {
                // Discard the first line since that has information about the address
                // space of the process. Some files may be empty and will return ESRCH
                // so just terminate early in that case.
                let mut first_line = String::new();
                if reader.read_line(&mut first_line).is_err() {
                    stream.write_all(footer.as_bytes())?;
                    return Ok(());
                }
            }

            if let Err(e) = std::io::copy(&mut reader, stream) {
                stream.write_all(format!("failed to read {}: {:?}", &path_str, e).as_bytes())?;
            }
        }
        Err(e) => {
            stream.write_all(format!("failed to open {}: {:?}", &path_str, e).as_bytes())?;
        }
    }

    stream.write_all(footer.as_bytes())?;

    Ok(())
}

fn read_and_write_glob_files(
    stream: &mut VsockStream,
    pattern: &str,
    filter: &[&str],
) -> Result<()> {
    for entry in glob(pattern).unwrap() {
        match entry {
            Ok(path) => {
                if path.starts_with("/proc/self")
                    || path.starts_with("/proc/thread-self")
                    || filter.iter().any(|x| path.to_string_lossy().contains(x))
                {
                    continue;
                }
                read_and_write_file(stream, &path)?;
            }
            Err(e) => {
                stream.write_all(format!("glob error for {pattern}: {e:?}\n").as_bytes())?;
            }
        }
    }

    Ok(())
}

fn handle_dump_to_client(mut stream: VsockStream) -> Result<()> {
    // 5 seconds must be much longer than required.
    stream.set_write_timeout(Some(Duration::from_secs(5))).context("Failed to set read timeout")?;

    read_and_write_file(&mut stream, &PathBuf::from("/proc/meminfo"))?;
    read_and_write_glob_files(&mut stream, "/proc/pressure/*", &[])?;
    read_and_write_glob_files(&mut stream, "/sys/fs/cgroup/*/memory.*", &["memory.reclaim"])?;
    read_and_write_glob_files(&mut stream, "/proc/*/smaps_rollup", &[])?;
    // Useful for understanding the amount of higher order pages in the system.
    read_and_write_file(&mut stream, &PathBuf::from("/proc/pagetypeinfo"))?;
    // Useful for global memory management stats.
    read_and_write_file(&mut stream, &PathBuf::from("/proc/vmstat"))?;
    // Useful for per-zone stats (e.g. watermarks).
    read_and_write_file(&mut stream, &PathBuf::from("/proc/zoneinfo"))?;

    if is_debuggable() {
        read_and_write_glob_files(&mut stream, "/proc/*/smaps", &[])?;
    }

    stream.shutdown(Shutdown::Write).context("Failed to shutdown")?;

    Ok(())
}
