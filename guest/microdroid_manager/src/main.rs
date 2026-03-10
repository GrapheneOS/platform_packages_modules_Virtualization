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
mod encrypted_assets;
mod encrypted_folders;
mod encrypted_store_kek;
mod instance;
mod ioutil;
mod payload;
mod swap;
mod tenant;
mod tenant_config;
mod verify;
mod vm_internal_service;
mod vm_payload_service;
mod vm_secret;

use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::{
    ErrorCode::ErrorCode,
    Atom::Atom,
    Atom::StaleEncryptedstoreDetected::StaleEncryptedstoreDetected,
    ICEStoreKEK::ICEStoreKEK,
    IGuestAgent::BnGuestAgent, IGuestAgent::IGuestAgent,
};
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use android_system_virtualization_internal::aidl::android::system::virtualization::internal::IVmInternalService::{
    BnVmInternalService, VM_INTERNAL_SERVICE_SOCKET_NAME,
};
use android_system_virtualization_payload::aidl::android::system::virtualization::payload::IVmPayloadService::{
    BnVmPayloadService,
    VM_APK_CONTENTS_PATH,
    VM_PAYLOAD_SERVICE_SOCKET_NAME,
    ENCRYPTEDSTORE_MOUNTPOINT,
    ENCRYPTEDSTORE_PER_USER_FOLDERS
};

use crate::cgroup_monitor::start_cgroup_monitor;
use crate::dice::dice_derivation;
use crate::encrypted_folders::set_encryption_key;
use crate::encrypted_store_kek::{decrypt_kek, encrypt_kek};
use crate::instance::{ApexData, EncryptedStoreMode, InstanceDisk, MicrodroidData};
use crate::tenant::{TenantAttribute, TenantManager};
use crate::tenant_config::validate_tenants_against_tenant_config;
use crate::verify::{integrity_protect_tenant_apks, verify_payload};
use crate::vm_internal_service::VmInternalService;
use crate::vm_payload_service::{VmPayloadService, VmPayloadServiceShared};
use anyhow::{anyhow, bail, ensure, Context, Error, Result};
use binder::{self, BinderFeatures, ExceptionCode, Interface, IntoBinderResult, SpIBinder, Strong};
use dice_driver::DiceDriver;
use encryptedstore_query::needs_formatting;
use glob::glob;
use keystore2_crypto::ZVec;
use libc::{VMADDR_CID_HOST, VMADDR_PORT_ANY};
use log::{error, info, warn};
use microdroid_metadata::{Metadata, PayloadMetadata};
use microdroid_payload_config::{
    ApkConfig, CgroupConfig, OsConfig, Task, TaskType, TenantConfig, VmPayloadConfig,
};
use nix::mount::{umount2, MntFlags};
use nix::sys::signal::{
    pthread_sigmask, sigaction, SaFlags, SigAction, SigHandler, SigSet, SigmaskHow, Signal,
};
use nix::sys::signalfd::{SfdFlags, SignalFd};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use payload::load_metadata;
#[cfg(vm_to_host_services)]
use rpc_servicemanager::register_rpc_servicemanager;
use rpcbinder::{FileDescriptorTransportMode, RpcServer, RpcSession};
use rustutils::android::sockets::android_get_control_socket;
use rustutils::android::system_properties;
use rustutils::android::system_properties::PropertyWatcher;
use secretkeeper_comm::data_types::ID_SIZE;
use std::borrow::Cow::{Borrowed, Owned};
use std::collections::HashSet;
use std::env;
use std::ffi::{CStr, CString};
use std::fs::{self, create_dir, create_dir_all, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::raw::c_char;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::OwnedFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::ptr;
use std::str;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::thread::JoinHandle;
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
const ENCRYPTEDSTORE_DM_DEFAULT_KEYSIZE: usize = 64;
const ENCRYPTEDSTORE_KEKSIZE: usize = 32;

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
        Owned(format!("MICRODROID_UNKNOWN_RUNTIME_ERROR|{err:?}"))
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

// TODO: Use libselinux_rs.
fn setexeccon(selinux_domain: &CStr) -> Result<()> {
    // Safety: we pass non null pointer to the setexeccon call here which is guaranteed
    // to be valid after call to CStr::as_ptr() that always returns valid pointer for
    // the lifetime of CStr.
    let result = unsafe { selinux_bindgen::setexeccon(selinux_domain.as_ptr()) };
    if result != 0 {
        return Err(anyhow!(format!(
            "Failed to set SELinux security context. Error code: {}. Errno: {}",
            result,
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
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
            error!("failed to shutdown {e:?}");
        }
    }

    try_main().map_err(|e| {
        error!("Failed with {e:?}.");
        if let Err(e) = write_death_reason_to_serial(&e) {
            error!("Failed to write death reason {e:?}");
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

    // Manually log the panic message because we don't get tombstones for microdroid_manager
    // (crashdump isn't given permission to in the SELinux policy).
    std::panic::set_hook(Box::new(|panic_info| error!("{panic_info}")));

    info!("started.");

    let mut mask = SigSet::empty();
    mask.add(Signal::SIGCHLD);
    pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&mask), None)?;

    load_crashkernel_if_supported().context("Failed to load crashkernel")?;

    swap::init_swap().context("Failed to initialize swap")?;
    info!("swap enabled.");

    if let Err(e) = set_bail_on_out_of_puff() {
        warn!("failed to set bail_on_out_of_puff: {e:#}");
    }

    let service = get_vms_rpc_binder()
        .context("cannot connect to VirtualMachineService")
        .map_err(|e| MicrodroidError::FailedToConnectToVirtualizationService(e.to_string()))?;

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
                v => error!("task exited with exit code: {v}"),
            };
            if let Err(e) = post_payload_work() {
                error!(
                    "Failed to run post payload work. It is possible that certain tasks like \
                     syncing encrypted store might be incomplete. Error: {e:?}"
                );
            };

            info!("notifying payload finished");
            service.notifyPayloadFinished(code)?;
            Ok(())
        }
        Err(err) => {
            warn!("payload finished erroneously: {err:?}");
            let (error_code, message) = translate_error(&err);
            service.notifyError(error_code, &message)?;
            Err(err)
        }
    }
}

// Verify the payload. Additionally compare it against instance.img partition (if existing)
// OR create a new entry in the instance,img (returning a boolean to indicate is_new_instance).
fn verify_payload_with_instance_img(
    metadata: &Metadata,
    dice: &DiceDriver,
    instance: &mut InstanceDisk,
) -> Result<(MicrodroidData, Vec<ApexData>, bool)> {
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
    let (extracted_data, tenant_apex_data) = verify_payload(metadata, saved_data.as_ref())
        .context("Payload verification failed")
        .map_err(|e| MicrodroidError::PayloadVerificationFailed(format!("{e:?}")))?;

    // In case identity is ignored (by debug policy), we should reuse existing payload data, even
    // when the payload is changed. This is to keep the derived secret same as before.
    let (instance_data, newly_created) = if let Some(saved_data) = saved_data {
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
        (saved_data, /* newly_created */ false)
    } else {
        info!("Saving verified data.");
        instance
            .write_microdroid_data(&extracted_data, dice)
            .context("Failed to write identity data")?;
        (extracted_data, /* newly_created */ true)
    };
    Ok((instance_data, tenant_apex_data, newly_created))
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

struct EncryptedstoreHandle {
    encryptedstore_thread: Option<JoinHandle<()>>,
}

impl Drop for EncryptedstoreHandle {
    fn drop(&mut self) {
        if let Some(t) = self.encryptedstore_thread.take() {
            if system_properties::read_bool(ENCRYPTED_STORE_SETUP_PROP, false).unwrap_or(false) {
                t.join().unwrap();
            }
        }
    }
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

    let mut instance_disk = InstanceDisk::new().context("Failed to load instance.img")?;

    // Microdroid skips checking payload against instance image iff the device supports
    // secretkeeper. In that case Microdroid use VmSecret::V2, which provides instance state
    // and protection against rollback of boot images and packages.
    let (instance_data, tenant_apex_data, state) = if should_defer_rollback_protection() {
        let (instance_data, tenant_apex_data) = verify_payload(&metadata, None)?;
        (instance_data, tenant_apex_data, VmInstanceState::Unknown)
    } else {
        let (instance_data, tenant_apex_data, is_newly_created) =
            verify_payload_with_instance_img(&metadata, &dice, &mut instance_disk)?;
        (
            instance_data,
            tenant_apex_data,
            if is_newly_created {
                VmInstanceState::NewlyCreated
            } else {
                VmInstanceState::PreviouslySeen
            },
        )
    };
    let tenant_apks = integrity_protect_tenant_apks()?;

    let payload_metadata = metadata.payload.ok_or_else(|| {
        MicrodroidError::PayloadInvalidConfig("No payload config in metadata".to_string())
    })?;
    // To minimize the exposure to untrusted data, derive dice profile as soon as possible.
    info!("DICE derivation for payload");
    let dice_artifacts =
        dice_derivation(dice, &instance_data, &payload_metadata, &tenant_apks, &tenant_apex_data)?;
    let (vm_secret, is_new_instance) =
        VmSecret::new(dice_artifacts, service, state).context("Failed to create VM secrets")?;
    let vm_secret = Arc::new(vm_secret);

    let guest_agent = GuestAgent::new_binder(vm_secret.clone());
    service.registerGuestAgent(&guest_agent)?;

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
    let package_name = instance_data.apk_data.package_name;

    // Before adding a cgroup API, we were checking if the rollback_index field existed to
    // configure cgroups.
    let (cgroup_name, cgroup_config) = if config.cgroup_config.is_some() {
        (package_name, config.cgroup_config.clone())
    } else if instance_data.apk_data.rollback_index.is_some() {
        (
            "microdroid_launcher".to_string(),
            Some(CgroupConfig { memory_high_mib: 50, increase_high_mib: true }),
        )
    } else {
        ("".to_string(), None)
    };

    if let Some(cgroup_config) = cgroup_config.as_ref() {
        // We create and configure the cgroup now, then the child process adds itself to the group
        // before `exec`ing the payload binary (see `exec_task` code).
        let cgroup_dir = std::path::Path::new("/sys/fs/cgroup").join(&cgroup_name);
        std::fs::create_dir(&cgroup_dir).context("failed to create cgroup dir")?;
        std::fs::write(
            cgroup_dir.join("memory.high"),
            format!("{}M", cgroup_config.memory_high_mib),
        )
        .context("failed to set cgroup memory.high")?;

        // Spawn thread to monitor the cgroup's behavior.
        // TODO: (khei@)
        // Send cgroup kill signal and join cgroup thread for graceful shutdown
        let (_cgroup_thread, _cgroup_kill) = start_cgroup_monitor(&cgroup_name, service)?;
    }

    if !config.tenants.is_empty() {
        validate_tenants_against_tenant_config(&tenant_apks, &tenant_apex_data, &config.tenants)?;
    }

    if cfg!(dice_changes) {
        // Now that the DICE derivation is done, it's ok to allow payload code to run.

        // Start apexd to activate APEXes. This may allow code within them to run.
        system_properties::write("ctl.start", "apexd-vm")?;

        // Unmounting /microdroid_resources is a defence-in-depth effort to ensure that payload
        // can't get hold of dice chain stored there.
        umount2("/microdroid_resources", MntFlags::MNT_DETACH)?;
    }

    let task = config.task.as_ref();
    let has_tenant_with_task = config.tenants.iter().any(|t| match t {
        TenantConfig::Apex(c) => c.task.is_some(),
        TenantConfig::Apk(c) => c.task.is_some(),
    });

    if task.is_some() && has_tenant_with_task {
        bail!(MicrodroidError::PayloadInvalidConfig(
            "Both main task and tenant task are present. Only one type is allowed.".to_string()
        ));
    }
    if task.is_none() && !has_tenant_with_task {
        bail!(MicrodroidError::PayloadInvalidConfig(
            "No task in VM config and no tenants with a task".to_string()
        ));
    }

    ensure!(
        config.extra_apks.len() == instance_data.extra_apks_data.len(),
        "config expects {} extra apks, but found {}",
        config.extra_apks.len(),
        instance_data.extra_apks_data.len()
    );
    mount_extra_apks(&mut zipfuse, config.extra_apks.len())
        .context("Failed to mount extra apks")?;

    // TODO(b/429639517): Verify the tenant packages against`VmPayloadConfig` from main_apk

    let tenant_manager = TenantManager::initialize(&config.tenants)?;
    let tenant_manager = Arc::new(tenant_manager);

    let tenant_apk_names: Vec<String> = config
        .tenants
        .iter()
        .filter_map(|t| if let TenantConfig::Apk(c) = t { Some(c.name.clone()) } else { None })
        .collect();
    mount_tenant_apks(&mut zipfuse, &tenant_apk_names).context("Failed to mount tenant apks")?;

    // Wait until apex config is done. (e.g. linker configuration for apexes)
    wait_for_property_true(APEX_CONFIG_DONE_PROP).context("Failed waiting for apex config done")?;

    let vm_internal_binder = BnVmInternalService::new_binder(
        VmInternalService::new(service.clone()),
        BinderFeatures::default(),
    );

    spawn_binder_rpc_server(
        vm_internal_binder.as_binder(),
        vm_internal_service_fd,
        VM_INTERNAL_SERVICE_SOCKET_NAME,
        /* enable_fd_transport */ false,
    )?;

    let mut encryptedstore_handle = EncryptedstoreHandle { encryptedstore_thread: None };
    // Run encryptedstore binary to prepare the storage
    // Postpone initialization until apex mount completes to ensure e2fsck and resize2fs binaries
    // are accessible.
    let encryptedstore_child = if Path::new(ENCRYPTEDSTORE_BACKING_DEVICE).exists() {
        let disk_is_new = needs_formatting(Path::new(ENCRYPTEDSTORE_BACKING_DEVICE))
            .context("failed to check if device formatted")?;
        if is_new_instance && !disk_is_new {
            if let Err(statsd_e) = service
                .forwardAtom(&Atom::StaleEncryptedstoreDetected(StaleEncryptedstoreDetected {}))
            {
                error!("Failed to report StaleEncryptedstore: {statsd_e}");
            }
            bail!(MicrodroidError::PayloadInvalidConfig(
                "InvalidKey: Unable to prepare encrypted storage.\
                    Detected stale encryptedstore whilst VM is new (with new keys)."
                    .to_string()
            ));
        }

        let key_size = if config.dm_default_key {
            ENCRYPTEDSTORE_DM_DEFAULT_KEYSIZE
        } else {
            ENCRYPTEDSTORE_KEYSIZE
        };

        if config.delay_encrypted_store_setup {
            let service_clone = service.clone();
            let vm_secret_for_enc_store = vm_secret.clone();
            let encrypted_store_mode = instance_data.apk_data.encrypted_store_mode;
            let tenant_manager_for_enc_store = tenant_manager.clone();
            info!("Delaying preparation of encryptedstore as requested ...");
            encryptedstore_handle.encryptedstore_thread = Some(std::thread::spawn(move || {
                if let Err(e) = delayed_prepare_encryptedstore(
                    encrypted_store_mode,
                    service_clone,
                    vm_secret_for_enc_store,
                    tenant_manager_for_enc_store,
                    // Encrytedstore disk has never been setup - force provision a new KEK!
                    key_size,
                    disk_is_new, // provision_new_key,
                    config.dm_default_key,
                ) {
                    // Ideally we'd communicate this back to the main thread and error out in a
                    // similar manner to the `!delayed_prepare_encryptedstore` case, but, for now,
                    // keep it simple and just SIGABRT.
                    panic!("delayed prepare encrypted store failed: {e:#?}");
                }
            }));
            None
        } else {
            info!("Preparing encryptedstore ...");
            let mut key = ZVec::new(key_size)?;
            vm_secret.derive_encryptedstore_key(&mut key).context("derive encrypted store key")?;
            Some(
                prepare_encryptedstore(&key, &tenant_manager, config.dm_default_key)
                    .context("encryptedstore run")?,
            )
        }
    } else {
        None
    };

    let total_tasks = config.task.is_some() as usize
        + config
            .tenants
            .iter()
            .filter(|t| match t {
                TenantConfig::Apex(c) => c.task.is_some(),
                TenantConfig::Apk(c) => c.task.is_some(),
            })
            .count();

    let shared_state = Arc::new(VmPayloadServiceShared {
        virtual_machine_service: service.clone(),
        allow_restricted_apis,
        secret: vm_secret.clone(),
        is_new_instance,
        total_tasks,
        tasks_ready: AtomicUsize::new(0),
        tenant_manager: tenant_manager.clone(),
    });

    let server =
        RpcServer::new_bound_socket_with_factory(vm_payload_service_fd, move |session, _| {
            let client_uid = session.get_client_uid();
            if client_uid.is_none() {
                error!("Failed to get client UID for RpcSession.");
            }

            info!("New client connected to VmPayloadService with UID: {:?}", client_uid);

            let service = VmPayloadService::new(shared_state.clone(), client_uid);
            Some(BnVmPayloadService::new_binder(service, BinderFeatures::default()).as_binder())
        })?;

    run_rpc_server(server, VM_PAYLOAD_SERVICE_SOCKET_NAME, /* enable_fd_transport */ true);

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

    // TODO(b/434925716): Remove notified_payload_started once we handle per tenant notifications
    let mut notified_payload_started = task.is_some();
    let mut payload_process = if let Some(task) = task {
        info!("boot completed, time to run payload");
        let main_command = get_task_command(
            VM_APK_CONTENTS_PATH,
            task,
            /* is_apex */ false,
            config.run_as_root,
        )
        .context("Failed to find payload")?;
        Some(
            exec_task(
                main_command,
                &cgroup_name,
                cgroup_config.as_ref(),
                service,
                /* notify_payload_started */ true,
            )
            .context("Failed to run payload")?,
        )
    } else {
        None
    };

    let mut tenant_processes: Vec<Child> = Vec::new();
    ensure!(
        config.tenants.is_empty() || !config.run_as_root,
        "Run as root not supported with tenants"
    );
    for tenant in config.tenants.iter() {
        let (task, name, package_path, is_apex) = match tenant {
            TenantConfig::Apex(c) => (c.task.as_ref(), &c.name, c.name.clone(), true),
            TenantConfig::Apk(c) => {
                let mnt_dir = format!("/mnt/tenant-apk/{}", c.name);
                (c.task.as_ref(), &c.name, mnt_dir, false)
            }
        };

        if let Some(task) = task {
            let tenant_attribute = tenant_manager
                .get_tenant_attribute(name)
                .with_context(|| format!("Failed to get tenant attribute for '{name}'"))?;
            let uid_gid = Some((tenant_attribute.uid(), TenantAttribute::gid()));
            let command = build_payload_command(
                package_path.as_ref(),
                task,
                uid_gid,
                tenant_attribute.selinux_domain(),
                is_apex,
            )
            .context(format!("Failed to build tenant {name} payload command"))?;
            let tenant_process = exec_task(
                command,
                &cgroup_name,
                cgroup_config.as_ref(),
                service,
                /* notify_payload_started */ !notified_payload_started,
            )
            .context("Failed to run tenant")?;
            notified_payload_started = true;
            if payload_process.is_none() {
                payload_process = Some(tenant_process);
            } else {
                tenant_processes.push(tenant_process);
            }
        }
    }

    setup_ignore_sigterm()?;

    // We need to wait for processes to finish asynchronously, to avoid zombies.
    let mut pids_to_reap: HashSet<Pid> =
        tenant_processes.iter().map(|p| Pid::from_raw(p.id() as i32)).collect();
    let payload_pid =
        Pid::from_raw(payload_process.as_ref().expect("payload process not set").id() as i32);
    pids_to_reap.insert(payload_pid);

    let exit_status = wait_for_all_processes(&mut pids_to_reap, payload_pid)?;
    get_payload_exit_code(exit_status)
}

// TODO(b/434925716): Report exit status of all the tenants back to the host
/// Wait for all processes in `pids_to_reap` to exit.
/// It returns the `WaitStatus` of the first processes that exits unsuccessfully,
/// if all the processes exits successfully then it returns the status of the first tenant
fn wait_for_all_processes(pids_to_reap: &mut HashSet<Pid>, payload_pid: Pid) -> Result<WaitStatus> {
    let mut payload_exit_status: Option<WaitStatus> = None;
    let mut first_failure: Option<WaitStatus> = None;

    let mut mask = SigSet::empty();
    mask.add(Signal::SIGCHLD);
    let sfd = SignalFd::with_flags(&mask, SfdFlags::SFD_CLOEXEC)?;

    while !pids_to_reap.is_empty() {
        // Wait for a SIGCHLD signal
        sfd.read_signal()?;

        // Linux's signal handling mechanism coalesces multiple signals of the same type into
        // a single event, if they occur in quick succession. So there can be instances when
        // more than 1 SIGCHLD event occurs but read_signal only receives 1.
        // Thus, iteratively check for pids that we manage and reap the ones that have exited
        for pid in pids_to_reap.clone().into_iter() {
            match waitpid(pid, Some(WaitPidFlag::WNOHANG)) {
                Ok(wait_status) => {
                    let is_failure = match wait_status {
                        WaitStatus::Exited(_, exit_code) => Some(exit_code != 0),
                        WaitStatus::Signaled(_, signal, _) => {
                            // If this tenant was signaled (and it's not a SIGTERM which is a clean
                            // shutdown)
                            Some(signal != Signal::SIGTERM)
                        }
                        // StillAlive, Stopped, Continued, etc. are ignored
                        _ => None,
                    };

                    if let Some(is_failure) = is_failure {
                        info!("Process {pid} exited with {wait_status:?}");
                        pids_to_reap.remove(&pid);
                        if pid == payload_pid {
                            payload_exit_status = Some(wait_status);
                        }
                        if is_failure && first_failure.is_none() {
                            first_failure = Some(wait_status);
                        }
                    }
                }
                Err(nix::errno::Errno::ECHILD) => {
                    // This can happen if another thread reaps the process
                    warn!("Tracked process {pid} was already reaped.");
                    pids_to_reap.remove(&pid);
                }
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Failed to wait for child process {pid}"));
                }
            }
        }
    }

    // Return the first unsuccessful tenant status if any, otherwise return the payload_exit_status
    first_failure
        .or(payload_exit_status)
        .ok_or_else(|| anyhow!("Payload process hasn't exited or status not saved"))
}

fn get_payload_exit_code(wait_status: WaitStatus) -> Result<i32> {
    match wait_status {
        WaitStatus::Exited(_, exit_code) => Ok(exit_code),
        WaitStatus::Signaled(_, signal, _) => {
            if signal == Signal::SIGTERM {
                info!("payload exited with SIGTERM");
                Ok(0)
            } else {
                Err(anyhow!("Payload exited due to signal: {} ({})", signal as i32, signal))
            }
        }
        _ => Err(anyhow!("Payload has neither exit code nor signal")),
    }
}

fn spawn_binder_rpc_server(
    binder: SpIBinder,
    fd: OwnedFd,
    name: &str,
    enable_fd_transport: bool,
) -> Result<()> {
    let server = RpcServer::new_bound_socket(binder, fd)?;
    run_rpc_server(server, name, enable_fd_transport);
    Ok(())
}

fn run_rpc_server(server: RpcServer, name: &str, enable_fd_transport: bool) {
    info!("The RPC server '{name}' is running.");
    // Required for the FD being passed through vm_payload_service to the payloads.
    if enable_fd_transport {
        server.set_supported_file_descriptor_transport_modes(&[FileDescriptorTransportMode::Unix]);
    }
    std::thread::spawn(move || {
        server.join();
    });
}

fn post_payload_work() -> Result<()> {
    // Sync the encrypted storage filesystem (flushes the filesystem caches).
    if Path::new(ENCRYPTEDSTORE_BACKING_DEVICE).exists() {
        use nix::fcntl::OFlag;
        let dirfd = nix::fcntl::open(
            ENCRYPTEDSTORE_MOUNTPOINT,
            OFlag::O_DIRECTORY | OFlag::O_RDONLY | OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::empty(),
        )
        .with_context(|| "Unable to open {ENCRYPTEDSTORE_MOUNTPOINT}")?;
        nix::unistd::syncfs(dirfd).context("failed to sync encrypted storage")?;
    }
    Ok(())
}

fn mount_tenant_apks(zipfuse: &mut Zipfuse, tenant_apk_names: &[String]) -> Result<()> {
    for (i, name) in tenant_apk_names.iter().enumerate() {
        let (option, dev, mount_dir, ready_prop) = (
            "fscontext=u:object_r:zipfusefs:s0,context=u:object_r:tenant_apk_file:s0",
            PathBuf::from(format!("/dev/block/mapper/tenant-apk-{i}")),
            PathBuf::from(format!("/mnt/tenant-apk/{}", name)),
            format!("microdroid_manager.tenant_apk.mounted.{name}"),
        );
        create_dir(&mount_dir).context("Failed to create mount dir for additional apks")?;

        // These run asynchronously in parallel - we wait later for them to complete.
        zipfuse.mount(option, &dev, &mount_dir, ready_prop)?;
    }
    Ok(())
}

fn mount_extra_apks(zipfuse: &mut Zipfuse, apk_count: usize) -> Result<()> {
    // For now, only the number of apks is important, as the mount point and dm-verity name is fixed
    for i in 0..apk_count {
        let (option, dev, mount_dir, ready_prop) = (
            "fscontext=u:object_r:zipfusefs:s0,context=u:object_r:extra_apk_file:s0",
            PathBuf::from(format!("/dev/block/mapper/extra-apk-{i}")),
            PathBuf::from(format!("/mnt/extra-apk/{i}")),
            format!("microdroid_manager.extra_apk.mounted.{i}"),
        );
        create_dir(&mount_dir).context("Failed to create mount dir for additional apks")?;

        // These run asynchronously in parallel - we wait later for them to complete.
        zipfuse.mount(option, &dev, &mount_dir, ready_prop)?;
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
            info!("loading config from {path:?}...");
            let file = ioutil::wait_for_file(path, WAIT_TIMEOUT)
                .with_context(|| format!("Failed to read {path:?}"))?;
            Ok(serde_json::from_reader(file)?)
        }
        PayloadMetadata::Config(payload_config) => {
            let task = Task {
                type_: TaskType::MicrodroidLauncher,
                command: payload_config.payload_binary_name,
                command_args: None,
                selinux_type: None,
            };
            // We don't care about the paths, only the number of extra APKs really matters.
            let extra_apks = (0..payload_config.extra_apk_count)
                .map(|i| ApkConfig { path: format!("extra-apk-{i}") })
                .collect();
            Ok(VmPayloadConfig {
                os: OsConfig { name: "microdroid".to_owned() },
                task: Some(task),
                extra_apks,
                // Tenants are only supported through config.json files
                tenants: vec![],
                delay_encrypted_store_setup: payload_config.delay_encrypted_store_setup,
                dm_default_key: payload_config.dm_default_key,
                ..Default::default()
            })
        }
        _ => bail!("Failed to match config against a config type."),
    }
}

/// Loads the crashkernel into memory using kexec if debuggable or debug policy says so.
/// The VM should be loaded with `crashkernel=' parameter in the cmdline to allocate memory
/// for crashkernel.
fn load_crashkernel_if_supported() -> Result<()> {
    let allocated = std::fs::read_to_string("/proc/cmdline")?.contains(" crashkernel=");
    if !allocated {
        info!("memory for crashkernel is not allocated");
        return Ok(());
    }

    let requested = is_debuggable();
    let forced = get_debug_policy_bool(AVF_DEBUG_POLICY_RAMDUMP);
    if !(requested || forced) {
        info!("memory for crashkernel is allocated but ramdump is not required");
        return Ok(());
    }

    let status = Command::new("/system/bin/kexec_load").status()?;
    if status.success() {
        info!("crashkernel for ramdump is loaded: requested={requested}, forced={forced}");
    } else if status.code() == Some(libc::ENOSYS) {
        warn!("crashkernel for ramdump is not supported");
    } else {
        return Err(anyhow!("crashkernel for ramdump failed to load: {status}"));
    }
    Ok(())
}

#[derive(Debug)]
struct PayloadCommand {
    command: Command,
    uid_gid: Option<(u32, u32)>,
    selinux_domain: Option<CString>,
}

fn build_command(package_name: &str, task: &Task, is_apex: bool) -> Result<Command> {
    match task.type_ {
        TaskType::Executable => {
            let mut cmd = Command::new(&task.command);
            if let Some(args) = &task.command_args {
                cmd.args(args);
            }
            Ok(cmd)
        }
        TaskType::MicrodroidLauncher => {
            let mut cmd = Command::new("/system/bin/microdroid_launcher");
            cmd.arg(find_library_path(package_name, &task.command, is_apex)?);
            Ok(cmd)
        }
    }
}

fn build_payload_command(
    package_name: &str,
    task: &Task,
    uid_gid: Option<(u32, u32)>,
    selinux_domain: Option<CString>,
    is_apex: bool,
) -> Result<PayloadCommand> {
    let command = build_command(package_name, task, is_apex)?;
    Ok(PayloadCommand { command, uid_gid, selinux_domain })
}

fn get_task_command(
    package_name: &str,
    task: &Task,
    is_apex: bool,
    run_as_root: bool,
) -> Result<PayloadCommand> {
    let uid_gid = if run_as_root {
        None
    } else {
        match task.type_ {
            TaskType::Executable => {
                // TODO(b/297501338): Figure out how to handle non-root for system payloads.
                None
            }
            TaskType::MicrodroidLauncher => Some((
                microdroid_uids::MICRODROID_PAYLOAD_UID,
                microdroid_uids::MICRODROID_PAYLOAD_GID,
            )),
        }
    };
    build_payload_command(package_name, task, uid_gid, None, is_apex)
}

/// Executes the given task.
fn exec_task(
    payload_cmd: PayloadCommand,
    cgroup_name: &String,
    cgroup_config: Option<&CgroupConfig>,
    service: &Strong<dyn IVirtualMachineService>,
    notify_payload_started: bool,
) -> Result<Child> {
    info!("executing main task {:?}...", payload_cmd);
    let mut command = payload_cmd.command;
    let cgroup_path = if cgroup_config.is_some() {
        Some(format!("/sys/fs/cgroup/{}/cgroup.procs", cgroup_name))
    } else {
        None
    };

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
            if let Some((uid, gid)) = payload_cmd.uid_gid {
                nix::unistd::setgid(nix::unistd::Gid::from_raw(gid))
                    .unwrap_or_else(|_| std::process::abort());
                nix::unistd::setuid(nix::unistd::Uid::from_raw(uid))
                    .unwrap_or_else(|_| std::process::abort());
            }
            if let Some(selinux_domain) = &payload_cmd.selinux_domain {
                setexeccon(selinux_domain).unwrap_or_else(|_| std::process::abort());
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

    if notify_payload_started {
        info!("notifying payload started");
        service.notifyPayloadStarted()?;
    }

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

fn find_library_path(package_name: &str, lib_name: &str, is_apex: bool) -> Result<String> {
    let paths = if !is_apex {
        let mut watcher = PropertyWatcher::new("ro.product.cpu.abilist")?;
        let value = watcher.read(|_name, value| value.trim().to_string())?;
        let abi = value.split(',').next().ok_or_else(|| anyhow!("no abilist"))?;
        [
            format!("{package_name}/lib/{abi}/{lib_name}"),
            // TODO(b/372535544): standardize
            "/apex/com.android.appsearch/lib64/libicing_anywhere.so".to_string(),
        ]
    } else {
        [
            format!("/apex/{package_name}/lib64/{lib_name}"),
            "/apex/com.android.appsearch/lib64/libicing_anywhere.so".to_string(),
        ]
    };

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

fn format_tenant_dir_specs(tenant_manager: &TenantManager) -> String {
    let gid = TenantAttribute::gid();
    tenant_manager
        .list_tenants_info()
        .map(|(package_name, tenant_attribute)| {
            format!("{}:{}:{}", package_name, tenant_attribute.uid(), gid)
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn prepare_encryptedstore(
    key: &[u8],
    tenant_manager: &TenantManager,
    dm_default_key: bool,
) -> Result<Child> {
    let mut cmd = Command::new(ENCRYPTEDSTORE_BIN);
    cmd.arg("--blkdevice")
        .arg(ENCRYPTEDSTORE_BACKING_DEVICE)
        .arg("--key")
        .arg(hex::encode(key))
        .args(["--mountpoint", ENCRYPTEDSTORE_MOUNTPOINT]);

    let tenant_dir_specs = format_tenant_dir_specs(tenant_manager);
    if !tenant_dir_specs.is_empty() {
        cmd.args(["--config-dir", &tenant_dir_specs]);
    }

    if dm_default_key {
        cmd.arg("--dm-default-key");
    }
    cmd.spawn().context("encryptedstore failed")
}

/// Implementation of `IGuestAgent`
struct GuestAgent {
    vm_secret: Arc<VmSecret>,
}

impl Interface for GuestAgent {}

impl IGuestAgent for GuestAgent {
    fn startDumpVsockServer(&self, args: &[String]) -> binder::Result<i32> {
        info!("Default dump handler with args: {args:?}");
        start_dump_service().or_service_specific_exception(-1)
    }

    fn shutdownAsync(&self) -> binder::Result<()> {
        info!("Shutdown requested.");
        if let Err(e) = system_properties::write("sys.powerctl", "shutdown") {
            error!("failed to shutdown {e:?}");
        }
        Ok(())
    }

    fn trimAsync(&self) -> binder::Result<()> {
        if let Err(e) = system_properties::write("pageout_bomb.go", "1") {
            error!("failed to set pageout_bomb.go: {e:?}");
        }
        Ok(())
    }

    fn userUnlocked(&self, user_id: i32, kek: &Strong<dyn ICEStoreKEK>) -> binder::Result<()> {
        let user_path = format!("{}/{user_id}", ENCRYPTEDSTORE_PER_USER_FOLDERS);
        let user_path = Path::new(&user_path);
        if let Err(e) = create_dir_all(user_path) {
            return Err(anyhow!("Cannot create {} {e}", user_path.display()))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }

        set_encrypted_store_per_user_key(kek, &self.vm_secret, user_path)
            .context("Failed to set per user key")
            .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn startOrStopAdbd(&self, start: bool) -> binder::Result<()> {
        if !system_properties::read_bool("init_debug_policy.adbd.enabled", false).unwrap_or(false) {
            return Err(anyhow!("adbd is not enabled"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }
        if start {
            system_properties::write("ctl.start", "adbd")
                .context("failed to start adbd")
                .or_service_specific_exception(-1)
        } else {
            system_properties::write("ctl.stop", "adbd")
                .context("failed to stop adbd")
                .or_service_specific_exception(-1)
        }
    }
}

fn set_encrypted_store_per_user_key(
    es_kek: &Strong<dyn ICEStoreKEK>,
    vm_secret: &VmSecret,
    user_path: &Path,
) -> Result<()> {
    let mut encryption_key = ZVec::new(ENCRYPTEDSTORE_KEKSIZE)?;
    vm_secret
        .derive_encryptedstore_key_encryption_key(&mut encryption_key)
        .context("failed to derive encryptedstore_key encryption key")?;

    let key = match es_kek.getKEK().context("failed to get KEK blob")? {
        Some(kek) => decrypt_kek(&kek, &encryption_key).context("failed to decrypt KEK blob")?,
        None => {
            let mut key = ZVec::new(ENCRYPTEDSTORE_KEYSIZE)?;
            vm_secret.derive_random_key(&mut key).context("derive random key")?;
            let encrypted_kek =
                encrypt_kek(&key, &encryption_key).context("failed to encrypt KEK")?;
            es_kek.onKEKCreated(&encrypted_kek).context("failed to send KEK blob to host")?;
            key
        }
    };
    set_encryption_key(user_path, &key)?;
    info!("Successfully unlocked per-user path {}", user_path.display());
    Ok(())
}

impl GuestAgent {
    fn new_binder(vm_secret: Arc<VmSecret>) -> Strong<dyn IGuestAgent> {
        BnGuestAgent::new_binder(GuestAgent { vm_secret }, BinderFeatures::default())
    }
}

fn delayed_prepare_encryptedstore(
    encrypted_store_mode: EncryptedStoreMode,
    service: Strong<dyn IVirtualMachineService>,
    vm_secret: Arc<VmSecret>,
    tenant_manager: Arc<TenantManager>,
    keysize: usize,
    provision_new_key: bool,
    dm_default_key: bool,
) -> Result<()> {
    info!("waiting for {ENCRYPTED_STORE_SETUP_PROP} to set up encrypted store");
    wait_for_property_true(ENCRYPTED_STORE_SETUP_PROP)
        .context("failed waiting for {ENCRYPTED_STORE_SETUP_PROP}")?;
    info!("{ENCRYPTED_STORE_SETUP_PROP} is true. Preparing encryptedstore ...");

    let mut key = ZVec::new(keysize)?;
    match encrypted_store_mode {
        EncryptedStoreMode::KEKsStoredOnHost => {
            encrypted_store_key(&service, &vm_secret, provision_new_key, &mut key)
                .context("KEK based encrypted store key setup failed")?;
        }
        EncryptedStoreMode::DefaultKey => {
            vm_secret.derive_encryptedstore_key(&mut key).context("derive encrypted store key")?;
        }
    }
    let exitcode = prepare_encryptedstore(&key, &tenant_manager, dm_default_key)?
        .wait()
        .context("failed waiting for encryptedstore binary to finish")?;
    ensure!(exitcode.success(), "Unable to prepare encrypted storage. Exitcode={}", exitcode);

    wait_for_property(ENCRYPTED_STORE_STATUS_PROP, "ready")
        .context("wait for {ENCRYPTED_STORE_STATUS_PROP}")?;

    // Now we can tell ueventd to stop.
    system_properties::write("microdroid_manager.init_done", "1")
        .context("set microdroid_manager.init_done")
}

fn encrypted_store_key(
    service: &Strong<dyn IVirtualMachineService>,
    vm_secret: &VmSecret,
    provision_new_key: bool,
    key: &mut [u8],
) -> Result<()> {
    let kek_wrapper =
        service.getEncryptedStoreKEK().context("failed to get host-side KEK handler")?;
    let kek_wrapper = if let Some(kek_wrapper) = kek_wrapper {
        kek_wrapper
    } else {
        bail!("expected encrypted store KEK handler from host but got nothing");
    };

    // This key is used to encrypt the key used for encrypted store setup.
    let mut encryption_key = ZVec::new(ENCRYPTEDSTORE_KEKSIZE)?;
    vm_secret
        .derive_encryptedstore_key_encryption_key(&mut encryption_key)
        .context("failed to derive encryptedstore_key encryption key")?;
    if provision_new_key {
        info!("Creating new KEK blob");
        vm_secret.derive_random_key(key).context("derive random key")?;
        let encrypted_kek = encrypt_kek(key, &encryption_key).context("failed to encrypt KEK")?;
        kek_wrapper.onKEKCreated(&encrypted_kek).context("failed to send KEK blob to host")?;
    } else {
        let kek = kek_wrapper.getKEK().context("failed to get KEK blob")?;
        let kek = kek.ok_or(anyhow!("Missing KEK blob"))?;
        let decrypted_key =
            decrypt_kek(&kek, &encryption_key).context("failed to decrypt KEK blob")?;
        key.copy_from_slice(&decrypted_key);
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
