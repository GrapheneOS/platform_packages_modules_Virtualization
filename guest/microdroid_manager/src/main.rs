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

mod dice;
mod instance;
mod ioutil;
mod payload;
mod swap;
mod verify;
mod vm_payload_service;
mod vm_secret;

use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::ErrorCode::ErrorCode;
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use android_system_virtualization_payload::aidl::android::system::virtualization::payload::IVmPayloadService::{
    VM_APK_CONTENTS_PATH,
    VM_PAYLOAD_SERVICE_SOCKET_NAME,
    ENCRYPTEDSTORE_MOUNTPOINT,
};

use crate::dice::dice_derivation;
use crate::instance::{InstanceDisk, MicrodroidData};
use crate::verify::verify_payload;
use crate::vm_payload_service::register_vm_payload_service;
use anyhow::{anyhow, bail, ensure, Context, Error, Result};
use binder::Strong;
use dice_driver::DiceDriver;
use keystore2_crypto::ZVec;
use libc::VMADDR_CID_HOST;
use log::{error, info};
use microdroid_metadata::{Metadata, PayloadMetadata};
use microdroid_payload_config::{ApkConfig, OsConfig, Task, TaskType, VmPayloadConfig};
use nix::mount::{umount2, MntFlags};
use nix::sys::signal::Signal;
use payload::load_metadata;
use rpcbinder::RpcSession;
use rustutils::sockets::android_get_control_socket;
use rustutils::system_properties;
use rustutils::system_properties::PropertyWatcher;
use safe_ownedfd::take_fd_ownership;
use secretkeeper_comm::data_types::ID_SIZE;
use std::borrow::Cow::{Borrowed, Owned};
use std::env;
use std::ffi::CString;
use std::fs::{self, create_dir, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::io::OwnedFd;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::str;
use std::time::Duration;
use vm_secret::VmSecret;

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

    for chunk in death_reason.as_bytes().chunks(16) {
        // TODO(b/220071963): Sometimes, sending more than 16 bytes at once makes MM hang.
        OpenOptions::new().read(false).write(true).open(FAILURE_SERIAL_DEVICE)?.write_all(chunk)?;
    }

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

fn main() -> Result<()> {
    // If debuggable, print full backtrace to console log with stdio_to_kmsg
    if is_debuggable()? {
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

    let vm_payload_service_fd = prepare_vm_payload_service_socket()?;

    load_crashkernel_if_supported().context("Failed to load crashkernel")?;

    swap::init_swap().context("Failed to initialize swap")?;
    info!("swap enabled.");

    let service = get_vms_rpc_binder()
        .context("cannot connect to VirtualMachineService")
        .map_err(|e| MicrodroidError::FailedToConnectToVirtualizationService(e.to_string()))?;

    match try_run_payload(&service, vm_payload_service_fd) {
        Ok(code) => {
            if code == 0 {
                info!("task successfully finished");
            } else {
                error!("task exited with exit code: {}", code);
            }
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
            let (error_code, message) = translate_error(&err);
            service.notifyError(error_code, &message)?;
            Err(err)
        }
    }
}

fn verify_payload_with_instance_img(
    metadata: &Metadata,
    dice: &DiceDriver,
) -> Result<MicrodroidData> {
    let mut instance = InstanceDisk::new().context("Failed to load instance.img")?;
    let saved_data = instance.read_microdroid_data(dice).context("Failed to read identity data")?;

    if is_strict_boot() {
        // Provisioning must happen on the first boot and never again.
        if is_new_instance() {
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
        saved_data
    } else {
        info!("Saving verified data.");
        instance
            .write_microdroid_data(&extracted_data, dice)
            .context("Failed to write identity data")?;
        extracted_data
    };
    Ok(instance_data)
}

fn try_run_payload(
    service: &Strong<dyn IVirtualMachineService>,
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

    // Microdroid skips checking payload against instance image iff the device supports
    // secretkeeper. In that case Microdroid use VmSecret::V2, which provide protection against
    // rollback of boot images and packages.
    let instance_data = if should_defer_rollback_protection() {
        verify_payload(&metadata, None)?
    } else {
        verify_payload_with_instance_img(&metadata, &dice)?
    };

    let payload_metadata = metadata.payload.ok_or_else(|| {
        MicrodroidError::PayloadInvalidConfig("No payload config in metadata".to_string())
    })?;

    // To minimize the exposure to untrusted data, derive dice profile as soon as possible.
    info!("DICE derivation for payload");
    let dice_artifacts = dice_derivation(dice, &instance_data, &payload_metadata)?;
    let vm_secret =
        VmSecret::new(dice_artifacts, service).context("Failed to create VM secrets")?;

    if cfg!(dice_changes) {
        // Now that the DICE derivation is done, it's ok to allow payload code to run.

        // Start apexd to activate APEXes. This may allow code within them to run.
        system_properties::write("ctl.start", "apexd-vm")?;

        // Unmounting /microdroid_resources is a defence-in-depth effort to ensure that payload
        // can't get hold of dice chain stored there.
        umount2("/microdroid_resources", MntFlags::MNT_DETACH)?;
    }

    // Run encryptedstore binary to prepare the storage
    let encryptedstore_child = if Path::new(ENCRYPTEDSTORE_BACKING_DEVICE).exists() {
        info!("Preparing encryptedstore ...");
        Some(prepare_encryptedstore(&vm_secret).context("encryptedstore run")?)
    } else {
        None
    };

    let mut zipfuse = Zipfuse::default();

    // Before reading a file from the APK, start zipfuse
    zipfuse.mount(
        MountForExec::Allowed,
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

    register_vm_payload_service(
        allow_restricted_apis,
        service.clone(),
        vm_secret,
        vm_payload_service_fd,
    )?;

    // Set export_tombstones if enabled
    if should_export_tombstones(&config) {
        // This property is read by tombstone_handler.
        system_properties::write("microdroid_manager.export_tombstones.enabled", "1")
            .context("set microdroid_manager.export_tombstones.enabled")?;
    }

    // Wait until apex config is done. (e.g. linker configuration for apexes)
    wait_for_property_true(APEX_CONFIG_DONE_PROP).context("Failed waiting for apex config done")?;

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
    }

    // Wait for init to have finished booting.
    wait_for_property_true("dev.bootcomplete").context("failed waiting for dev.bootcomplete")?;

    // And then tell it we're done so unnecessary services can be shut down.
    system_properties::write("microdroid_manager.init_done", "1")
        .context("set microdroid_manager.init_done")?;

    info!("boot completed, time to run payload");
    exec_task(task, service).context("Failed to run payload")
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

        let mount_for_exec =
            if cfg!(multi_tenant) { MountForExec::Allowed } else { MountForExec::Disallowed };
        // These run asynchronously in parallel - we wait later for them to complete.
        zipfuse.mount(
            mount_for_exec,
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
    RpcSession::new()
        .setup_vsock_client(VMADDR_CID_HOST, port)
        .context("Could not connect to IVirtualMachineService")
}

/// Prepares a socket file descriptor for the vm payload service.
fn prepare_vm_payload_service_socket() -> Result<OwnedFd> {
    let raw_fd = android_get_control_socket(VM_PAYLOAD_SERVICE_SOCKET_NAME)?;
    Ok(take_fd_ownership(raw_fd)?)
}

fn is_strict_boot() -> bool {
    Path::new(AVF_STRICT_BOOT).exists()
}

fn is_new_instance() -> bool {
    Path::new(AVF_NEW_INSTANCE).exists()
}

fn is_verified_boot() -> bool {
    !Path::new(DEBUG_MICRODROID_NO_VERIFIED_BOOT).exists()
}

fn is_debuggable() -> Result<bool> {
    Ok(system_properties::read_bool(DEBUGGABLE_PROP, true)?)
}

fn should_export_tombstones(config: &VmPayloadConfig) -> bool {
    match config.export_tombstones {
        Some(b) => b,
        None => is_debuggable().unwrap_or(false),
    }
}

/// Get debug policy value in bool. It's true iff the value is explicitly set to <1>.
fn get_debug_policy_bool(path: &'static str) -> Result<Option<bool>> {
    let mut file = match File::open(path) {
        Ok(dp) => dp,
        Err(e) => {
            info!(
                "Assumes that debug policy is disabled because failed to read debug policy ({e:?})"
            );
            return Ok(Some(false));
        }
    };
    let mut log: [u8; 4] = Default::default();
    file.read_exact(&mut log).context("Malformed data in {path}")?;
    // DT spec uses big endian although Android is always little endian.
    Ok(Some(u32::from_be_bytes(log) == 1))
}

enum MountForExec {
    Allowed,
    Disallowed,
}

#[derive(Default)]
struct Zipfuse {
    ready_properties: Vec<String>,
}

impl Zipfuse {
    fn mount(
        &mut self,
        noexec: MountForExec,
        option: &str,
        zip_path: &Path,
        mount_dir: &Path,
        ready_prop: String,
    ) -> Result<Child> {
        let mut cmd = Command::new(ZIPFUSE_BIN);
        if let MountForExec::Disallowed = noexec {
            cmd.arg("--noexec");
        }
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

    let debuggable = is_debuggable()?;
    let ramdump = get_debug_policy_bool(AVF_DEBUG_POLICY_RAMDUMP)?.unwrap_or_default();
    let requested = debuggable | ramdump;

    if requested {
        let status = Command::new("/system/bin/kexec_load").status()?;
        if !status.success() {
            return Err(anyhow!("Failed to load crashkernel: {:?}", status));
        }
        info!("ramdump is loaded: debuggable={debuggable}, ramdump={ramdump}");
    }
    Ok(())
}

/// Executes the given task.
fn exec_task(task: &Task, service: &Strong<dyn IVirtualMachineService>) -> Result<i32> {
    info!("executing main task {:?}...", task);
    let mut command = match task.type_ {
        TaskType::Executable => {
            // TODO(b/297501338): Figure out how to handle non-root for system payloads.
            Command::new(&task.command)
        }
        TaskType::MicrodroidLauncher => {
            let mut command = Command::new("/system/bin/microdroid_launcher");
            command.arg(find_library_path(&task.command)?);
            command.uid(microdroid_uids::MICRODROID_PAYLOAD_UID);
            command.gid(microdroid_uids::MICRODROID_PAYLOAD_GID);
            command
        }
    };

    // SAFETY: We are not accessing any resource of the parent process. This means we can't make any
    // log calls inside the closure.
    unsafe {
        command.pre_exec(|| {
            // It is OK to continue with payload execution even if the calls below fail, since
            // whether process can use a capability is controlled by the SELinux. Dropping the
            // capabilities here is just another defense-in-depth layer.
            let _ = cap::drop_inheritable_caps();
            let _ = cap::drop_bounding_set();
            Ok(())
        });
    }

    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    info!("notifying payload started");
    service.notifyPayloadStarted()?;

    let exit_status = command.spawn()?.wait()?;
    match exit_status.code() {
        Some(exit_code) => Ok(exit_code),
        None => Err(match exit_status.signal() {
            Some(signal) => anyhow!(
                "Payload exited due to signal: {} ({})",
                signal,
                Signal::try_from(signal).map_or("unknown", |s| s.as_str())
            ),
            None => anyhow!("Payload has neither exit code nor signal"),
        }),
    }
}

fn find_library_path(name: &str) -> Result<String> {
    let mut watcher = PropertyWatcher::new("ro.product.cpu.abilist")?;
    let value = watcher.read(|_name, value| Ok(value.trim().to_string()))?;
    let abi = value.split(',').next().ok_or_else(|| anyhow!("no abilist"))?;
    let path = format!("{}/lib/{}/{}", VM_APK_CONTENTS_PATH, abi, name);

    let metadata = fs::metadata(&path).with_context(|| format!("Unable to access {}", path))?;
    if !metadata.is_file() {
        bail!("{} is not a file", &path);
    }

    Ok(path)
}

fn prepare_encryptedstore(vm_secret: &VmSecret) -> Result<Child> {
    let mut key = ZVec::new(ENCRYPTEDSTORE_KEYSIZE)?;
    vm_secret.derive_encryptedstore_key(&mut key)?;
    let mut cmd = Command::new(ENCRYPTEDSTORE_BIN);
    cmd.arg("--blkdevice")
        .arg(ENCRYPTEDSTORE_BACKING_DEVICE)
        .arg("--key")
        .arg(hex::encode(&*key))
        .args(["--mountpoint", ENCRYPTEDSTORE_MOUNTPOINT])
        .spawn()
        .context("encryptedstore failed")
}
