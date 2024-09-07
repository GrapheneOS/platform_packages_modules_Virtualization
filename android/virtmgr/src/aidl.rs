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

//! Implementation of the AIDL interface of the VirtualizationService.

use crate::{get_calling_pid, get_calling_uid, get_this_pid};
use crate::atom::{write_vm_booted_stats, write_vm_creation_stats};
use crate::composite::make_composite_image;
use crate::crosvm::{AudioConfig, CrosvmConfig, DiskFile, DisplayConfig, GpuConfig, InputDeviceOption, PayloadState, UsbConfig, VmContext, VmInstance, VmState};
use crate::debug_config::DebugConfig;
use crate::dt_overlay::{create_device_tree_overlay, VM_DT_OVERLAY_MAX_SIZE, VM_DT_OVERLAY_PATH};
use crate::payload::{add_microdroid_payload_images, add_microdroid_system_images, add_microdroid_vendor_image};
use crate::selinux::{getfilecon, SeContext};
use android_os_permissions_aidl::aidl::android::os::IPermissionController;
use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::{
    Certificate::Certificate,
    DeathReason::DeathReason,
    ErrorCode::ErrorCode,
};
use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    AssignableDevice::AssignableDevice,
    CpuTopology::CpuTopology,
    DiskImage::DiskImage,
    InputDevice::InputDevice,
    IVirtualMachine::{BnVirtualMachine, IVirtualMachine},
    IVirtualMachineCallback::IVirtualMachineCallback,
    IVirtualizationService::IVirtualizationService,
    Partition::Partition,
    PartitionType::PartitionType,
    VirtualMachineAppConfig::{DebugLevel::DebugLevel, Payload::Payload, VirtualMachineAppConfig},
    VirtualMachineConfig::VirtualMachineConfig,
    VirtualMachineDebugInfo::VirtualMachineDebugInfo,
    VirtualMachinePayloadConfig::VirtualMachinePayloadConfig,
    VirtualMachineRawConfig::VirtualMachineRawConfig,
    VirtualMachineState::VirtualMachineState,
};
use android_system_virtualizationservice_internal::aidl::android::system::virtualizationservice_internal::IGlobalVmContext::IGlobalVmContext;
use android_system_virtualizationservice_internal::aidl::android::system::virtualizationservice_internal::IVirtualizationServiceInternal::IVirtualizationServiceInternal;
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IVirtualMachineService::{
        BnVirtualMachineService, IVirtualMachineService,
};
use android_hardware_security_secretkeeper::aidl::android::hardware::security::secretkeeper::ISecretkeeper::{BnSecretkeeper, ISecretkeeper};
use android_hardware_security_secretkeeper::aidl::android::hardware::security::secretkeeper::SecretId::SecretId;
use android_hardware_security_authgraph::aidl::android::hardware::security::authgraph::{
    Arc::Arc as AuthgraphArc, IAuthGraphKeyExchange::IAuthGraphKeyExchange,
    IAuthGraphKeyExchange::BnAuthGraphKeyExchange, Identity::Identity, KeInitResult::KeInitResult,
    Key::Key, PubKey::PubKey, SessionIdSignature::SessionIdSignature, SessionInfo::SessionInfo,
    SessionInitiationInfo::SessionInitiationInfo,
};
use anyhow::{anyhow, bail, Context, Result};
use apkverify::{HashAlgorithm, V4Signature};
use avflog::LogResult;
use binder::{
    self, wait_for_interface, BinderFeatures, ExceptionCode, Interface, ParcelFileDescriptor,
    Status, StatusCode, Strong,
    IntoBinderResult,
};
use cstr::cstr;
use glob::glob;
use log::{debug, error, info, warn};
use microdroid_payload_config::{ApkConfig, Task, TaskType, VmPayloadConfig};
use nix::unistd::pipe;
use rpcbinder::RpcServer;
use rustutils::system_properties;
use semver::VersionReq;
use serde::Deserialize;
use std::collections::HashSet;
use std::convert::TryInto;
use std::fs;
use std::ffi::CStr;
use std::fs::{canonicalize, create_dir_all, read_dir, remove_dir_all, remove_file, File, OpenOptions};
use std::io::{BufRead, BufReader, Error, ErrorKind, Seek, SeekFrom, Write};
use std::iter;
use std::num::{NonZeroU16, NonZeroU32};
use std::ops::Range;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::raw::pid_t;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak, LazyLock};
use vbmeta::VbMetaImage;
use vmconfig::{VmConfig, get_debug_level};
use vsock::VsockStream;
use zip::ZipArchive;

/// The unique ID of a VM used (together with a port number) for vsock communication.
pub type Cid = u32;

pub const BINDER_SERVICE_IDENTIFIER: &str = "android.system.virtualizationservice";

/// The size of zero.img.
/// Gaps in composite disk images are filled with a shared zero.img.
const ZERO_FILLER_SIZE: u64 = 4096;

/// Magic string for the instance image
const ANDROID_VM_INSTANCE_MAGIC: &str = "Android-VM-instance";

/// Version of the instance image format
const ANDROID_VM_INSTANCE_VERSION: u16 = 1;

const MICRODROID_OS_NAME: &str = "microdroid";

const SECRETKEEPER_IDENTIFIER: &str =
    "android.hardware.security.secretkeeper.ISecretkeeper/default";

const UNFORMATTED_STORAGE_MAGIC: &str = "UNFORMATTED-STORAGE";

/// crosvm requires all partitions to be a multiple of 4KiB.
const PARTITION_GRANULARITY_BYTES: u64 = 4096;

const VM_REFERENCE_DT_ON_HOST_PATH: &str = "/proc/device-tree/avf/reference";

pub static GLOBAL_SERVICE: LazyLock<Strong<dyn IVirtualizationServiceInternal>> =
    LazyLock::new(|| {
        if cfg!(early) {
            panic!("Early virtmgr must not connect to VirtualizatinoServiceInternal")
        } else {
            wait_for_interface(BINDER_SERVICE_IDENTIFIER)
                .expect("Could not connect to VirtualizationServiceInternal")
        }
    });
static SUPPORTED_OS_NAMES: LazyLock<HashSet<String>> =
    LazyLock::new(|| get_supported_os_names().expect("Failed to get list of supported os names"));

fn create_or_update_idsig_file(
    input_fd: &ParcelFileDescriptor,
    idsig_fd: &ParcelFileDescriptor,
) -> Result<()> {
    let mut input = clone_file(input_fd)?;
    let metadata = input.metadata().context("failed to get input metadata")?;
    if !metadata.is_file() {
        bail!("input is not a regular file");
    }
    let mut sig =
        V4Signature::create(&mut input, get_current_sdk()?, 4096, &[], HashAlgorithm::SHA256)
            .context("failed to create idsig")?;

    let mut output = clone_file(idsig_fd)?;

    // Optimization. We don't have to update idsig file whenever a VM is started. Don't update it,
    // if the idsig file already has the same APK digest.
    if output.metadata()?.len() > 0 {
        if let Ok(out_sig) = V4Signature::from_idsig(&mut output) {
            if out_sig.signing_info.apk_digest == sig.signing_info.apk_digest {
                debug!("idsig {:?} is up-to-date with apk {:?}.", output, input);
                return Ok(());
            }
        }
        // if we fail to read v4signature from output, that's fine. User can pass a random file.
        // We will anyway overwrite the file to the v4signature generated from input_fd.
    }

    output
        .seek(SeekFrom::Start(0))
        .context("failed to move cursor to start on the idsig output")?;
    output.set_len(0).context("failed to set_len on the idsig output")?;
    sig.write_into(&mut output).context("failed to write idsig")?;
    Ok(())
}

fn get_current_sdk() -> Result<u32> {
    let current_sdk = system_properties::read("ro.build.version.sdk")?;
    let current_sdk = current_sdk.ok_or_else(|| anyhow!("SDK version missing"))?;
    current_sdk.parse().context("Malformed SDK version")
}

pub fn remove_temporary_files(path: &PathBuf) -> Result<()> {
    for dir_entry in read_dir(path)? {
        remove_file(dir_entry?.path())?;
    }
    Ok(())
}

/// Implementation of `IVirtualizationService`, the entry point of the AIDL service.
#[derive(Debug, Default)]
pub struct VirtualizationService {
    state: Arc<Mutex<State>>,
}

impl Interface for VirtualizationService {
    fn dump(&self, writer: &mut dyn Write, _args: &[&CStr]) -> Result<(), StatusCode> {
        check_permission("android.permission.DUMP").or(Err(StatusCode::PERMISSION_DENIED))?;
        let state = &mut *self.state.lock().unwrap();
        let vms = state.vms();
        writeln!(writer, "Running {0} VMs:", vms.len()).or(Err(StatusCode::UNKNOWN_ERROR))?;
        for vm in vms {
            writeln!(writer, "VM CID: {}", vm.cid).or(Err(StatusCode::UNKNOWN_ERROR))?;
            writeln!(writer, "\tState: {:?}", vm.vm_state.lock().unwrap())
                .or(Err(StatusCode::UNKNOWN_ERROR))?;
            writeln!(writer, "\tPayload state {:?}", vm.payload_state())
                .or(Err(StatusCode::UNKNOWN_ERROR))?;
            writeln!(writer, "\tProtected: {}", vm.protected).or(Err(StatusCode::UNKNOWN_ERROR))?;
            writeln!(writer, "\ttemporary_directory: {}", vm.temporary_directory.to_string_lossy())
                .or(Err(StatusCode::UNKNOWN_ERROR))?;
            writeln!(writer, "\trequester_uid: {}", vm.requester_uid)
                .or(Err(StatusCode::UNKNOWN_ERROR))?;
            writeln!(writer, "\trequester_debug_pid: {}", vm.requester_debug_pid)
                .or(Err(StatusCode::UNKNOWN_ERROR))?;
        }
        Ok(())
    }
}
impl IVirtualizationService for VirtualizationService {
    /// Creates (but does not start) a new VM with the given configuration, assigning it the next
    /// available CID.
    ///
    /// Returns a binder `IVirtualMachine` object referring to it, as a handle for the client.
    fn createVm(
        &self,
        config: &VirtualMachineConfig,
        console_out_fd: Option<&ParcelFileDescriptor>,
        console_in_fd: Option<&ParcelFileDescriptor>,
        log_fd: Option<&ParcelFileDescriptor>,
    ) -> binder::Result<Strong<dyn IVirtualMachine>> {
        let mut is_protected = false;
        let ret = self.create_vm_internal(
            config,
            console_out_fd,
            console_in_fd,
            log_fd,
            &mut is_protected,
        );
        write_vm_creation_stats(config, is_protected, &ret);
        ret
    }

    /// Allocate a new instance_id to the VM
    fn allocateInstanceId(&self) -> binder::Result<[u8; 64]> {
        check_manage_access()?;
        GLOBAL_SERVICE.allocateInstanceId()
    }

    /// Initialise an empty partition image of the given size to be used as a writable partition.
    fn initializeWritablePartition(
        &self,
        image_fd: &ParcelFileDescriptor,
        size_bytes: i64,
        partition_type: PartitionType,
    ) -> binder::Result<()> {
        check_manage_access()?;
        let size_bytes = size_bytes
            .try_into()
            .with_context(|| format!("Invalid size: {}", size_bytes))
            .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT)?;
        let size_bytes = round_up(size_bytes, PARTITION_GRANULARITY_BYTES);
        let mut image = clone_file(image_fd)?;
        // initialize the file. Any data in the file will be erased.
        image
            .seek(SeekFrom::Start(0))
            .context("failed to move cursor to start")
            .or_service_specific_exception(-1)?;
        image.set_len(0).context("Failed to reset a file").or_service_specific_exception(-1)?;
        // Set the file length. In most filesystems, this will not allocate any physical disk
        // space, it will only change the logical size.
        image
            .set_len(size_bytes)
            .context("Failed to extend file")
            .or_service_specific_exception(-1)?;

        match partition_type {
            PartitionType::RAW => Ok(()),
            PartitionType::ANDROID_VM_INSTANCE => format_as_android_vm_instance(&mut image),
            PartitionType::ENCRYPTEDSTORE => format_as_encryptedstore(&mut image),
            _ => Err(Error::new(
                ErrorKind::Unsupported,
                format!("Unsupported partition type {:?}", partition_type),
            )),
        }
        .with_context(|| format!("Failed to initialize partition as {:?}", partition_type))
        .or_service_specific_exception(-1)?;

        Ok(())
    }

    /// Creates or update the idsig file by digesting the input APK file.
    fn createOrUpdateIdsigFile(
        &self,
        input_fd: &ParcelFileDescriptor,
        idsig_fd: &ParcelFileDescriptor,
    ) -> binder::Result<()> {
        check_manage_access()?;

        create_or_update_idsig_file(input_fd, idsig_fd).or_service_specific_exception(-1)?;
        Ok(())
    }

    /// Get a list of all currently running VMs. This method is only intended for debug purposes,
    /// and as such is only permitted from the shell user.
    fn debugListVms(&self) -> binder::Result<Vec<VirtualMachineDebugInfo>> {
        // Delegate to the global service, including checking the debug permission.
        GLOBAL_SERVICE.debugListVms()
    }

    /// Get a list of assignable device types.
    fn getAssignableDevices(&self) -> binder::Result<Vec<AssignableDevice>> {
        // Delegate to the global service, including checking the permission.
        GLOBAL_SERVICE.getAssignableDevices()
    }

    /// Get a list of supported OSes.
    fn getSupportedOSList(&self) -> binder::Result<Vec<String>> {
        Ok(Vec::from_iter(SUPPORTED_OS_NAMES.iter().cloned()))
    }

    /// Returns whether given feature is enabled
    fn isFeatureEnabled(&self, feature: &str) -> binder::Result<bool> {
        check_manage_access()?;
        Ok(avf_features::is_feature_enabled(feature))
    }

    fn enableTestAttestation(&self) -> binder::Result<()> {
        GLOBAL_SERVICE.enableTestAttestation()
    }

    fn isRemoteAttestationSupported(&self) -> binder::Result<bool> {
        check_manage_access()?;
        GLOBAL_SERVICE.isRemoteAttestationSupported()
    }

    fn isUpdatableVmSupported(&self) -> binder::Result<bool> {
        // The response is specific to Microdroid. Updatable VMs are only possible if device
        // supports Secretkeeper. Guest OS needs to use Secretkeeper based secrets. Microdroid does
        // this, however other guest OSes may do things differently.
        check_manage_access()?;
        Ok(is_secretkeeper_supported())
    }

    fn removeVmInstance(&self, instance_id: &[u8; 64]) -> binder::Result<()> {
        check_manage_access()?;
        GLOBAL_SERVICE.removeVmInstance(instance_id)
    }

    fn claimVmInstance(&self, instance_id: &[u8; 64]) -> binder::Result<()> {
        check_manage_access()?;
        GLOBAL_SERVICE.claimVmInstance(instance_id)
    }
}

/// Implementation of the AIDL `IGlobalVmContext` interface for early VMs.
#[derive(Debug, Default)]
struct EarlyVmContext {
    /// The unique CID assigned to the VM for vsock communication.
    cid: Cid,
    /// Temporary directory for this VM instance.
    temp_dir: PathBuf,
}

impl EarlyVmContext {
    fn new(cid: Cid, temp_dir: PathBuf) -> Result<Self> {
        // Remove the entire directory before creating a VM. Early VMs use predefined CIDs and AVF
        // should trust clients, e.g. they won't run two VMs at the same time
        let _ = remove_dir_all(&temp_dir);
        create_dir_all(&temp_dir).context(format!("can't create '{}'", temp_dir.display()))?;

        Ok(Self { cid, temp_dir })
    }
}

impl Interface for EarlyVmContext {}

impl Drop for EarlyVmContext {
    fn drop(&mut self) {
        if let Err(e) = remove_dir_all(&self.temp_dir) {
            error!("Cannot remove {} upon dropping: {e}", self.temp_dir.display());
        }
    }
}

impl IGlobalVmContext for EarlyVmContext {
    fn getCid(&self) -> binder::Result<i32> {
        Ok(self.cid as i32)
    }

    fn getTemporaryDirectory(&self) -> binder::Result<String> {
        Ok(self.temp_dir.to_string_lossy().to_string())
    }

    fn setHostConsoleName(&self, _pathname: &str) -> binder::Result<()> {
        Err(Status::new_exception_str(
            ExceptionCode::UNSUPPORTED_OPERATION,
            Some("Early VM doesn't support setting host console name"),
        ))
    }
}

fn find_partition(path: &Path) -> binder::Result<String> {
    match path.components().nth(1) {
        Some(std::path::Component::Normal(partition)) => {
            Ok(partition.to_string_lossy().into_owned())
        }
        _ => Err(anyhow!("Can't find partition in '{}'", path.display()))
            .or_service_specific_exception(-1),
    }
}

impl VirtualizationService {
    pub fn init() -> VirtualizationService {
        VirtualizationService::default()
    }

    fn create_early_vm_context(
        &self,
        config: &VirtualMachineConfig,
    ) -> binder::Result<(VmContext, Cid, PathBuf)> {
        let calling_exe_path = format!("/proc/{}/exe", get_calling_pid());
        let link = fs::read_link(&calling_exe_path)
            .context(format!("can't read_link '{calling_exe_path}'"))
            .or_service_specific_exception(-1)?;
        let partition = find_partition(&link)?;

        let name = match config {
            VirtualMachineConfig::RawConfig(config) => &config.name,
            VirtualMachineConfig::AppConfig(config) => &config.name,
        };
        let early_vm =
            find_early_vm_for_partition(&partition, name).or_service_specific_exception(-1)?;
        if Path::new(&early_vm.path) != link {
            return Err(anyhow!(
                "VM '{name}' in partition '{partition}' must be created with '{}', not '{}'",
                &early_vm.path,
                link.display()
            ))
            .or_service_specific_exception(-1);
        }

        let cid = early_vm.cid as Cid;
        let temp_dir = PathBuf::from(format!("/mnt/vm/early/{cid}"));

        let context = EarlyVmContext::new(cid, temp_dir.clone())
            .context(format!("Can't create early vm contexts for {cid}"))
            .or_service_specific_exception(-1)?;
        let service = VirtualMachineService::new_binder(self.state.clone(), cid).as_binder();

        // Start VM service listening for connections from the new CID on port=CID.
        let port = cid;
        let vm_server = RpcServer::new_vsock(service, cid, port)
            .context(format!("Could not start RpcServer on port {port}"))
            .or_service_specific_exception(-1)?;
        vm_server.start();
        Ok((VmContext::new(Strong::new(Box::new(context)), vm_server), cid, temp_dir))
    }

    fn create_vm_context(
        &self,
        requester_debug_pid: pid_t,
    ) -> binder::Result<(VmContext, Cid, PathBuf)> {
        const NUM_ATTEMPTS: usize = 5;

        for _ in 0..NUM_ATTEMPTS {
            let vm_context = GLOBAL_SERVICE.allocateGlobalVmContext(requester_debug_pid)?;
            let cid = vm_context.getCid()? as Cid;
            let temp_dir: PathBuf = vm_context.getTemporaryDirectory()?.into();
            let service = VirtualMachineService::new_binder(self.state.clone(), cid).as_binder();

            // Start VM service listening for connections from the new CID on port=CID.
            let port = cid;
            match RpcServer::new_vsock(service, cid, port) {
                Ok(vm_server) => {
                    vm_server.start();
                    return Ok((VmContext::new(vm_context, vm_server), cid, temp_dir));
                }
                Err(err) => {
                    warn!("Could not start RpcServer on port {}: {}", port, err);
                }
            }
        }
        Err(anyhow!("Too many attempts to create VM context failed"))
            .or_service_specific_exception(-1)
    }

    fn create_vm_internal(
        &self,
        config: &VirtualMachineConfig,
        console_out_fd: Option<&ParcelFileDescriptor>,
        console_in_fd: Option<&ParcelFileDescriptor>,
        log_fd: Option<&ParcelFileDescriptor>,
        is_protected: &mut bool,
    ) -> binder::Result<Strong<dyn IVirtualMachine>> {
        let requester_uid = get_calling_uid();
        let requester_debug_pid = get_calling_pid();

        check_config_features(config)?;

        if cfg!(early) {
            check_config_allowed_for_early_vms(config)?;
        }

        // Allocating VM context checks the MANAGE_VIRTUAL_MACHINE permission.
        let (vm_context, cid, temporary_directory) = if cfg!(early) {
            self.create_early_vm_context(config)?
        } else {
            self.create_vm_context(requester_debug_pid)?
        };

        if is_custom_config(config) {
            check_use_custom_virtual_machine()?;
        }

        let gdb_port = extract_gdb_port(config);

        // Additional permission checks if caller request gdb.
        if gdb_port.is_some() {
            check_gdb_allowed(config)?;
        }

        let device_tree_overlay = maybe_create_device_tree_overlay(config, &temporary_directory)?;

        let debug_config = DebugConfig::new(config);
        let ramdump = if !uses_gki_kernel(config) && debug_config.is_ramdump_needed() {
            Some(prepare_ramdump_file(&temporary_directory)?)
        } else {
            None
        };

        let state = &mut *self.state.lock().unwrap();
        let console_out_fd =
            clone_or_prepare_logger_fd(console_out_fd, format!("Console({})", cid))?;
        let console_in_fd = console_in_fd.map(clone_file).transpose()?;
        let log_fd = clone_or_prepare_logger_fd(log_fd, format!("Log({})", cid))?;

        // Counter to generate unique IDs for temporary image files.
        let mut next_temporary_image_id = 0;
        // Files which are referred to from composite images. These must be mapped to the crosvm
        // child process, and not closed before it is started.
        let mut indirect_files = vec![];

        let (is_app_config, config) = match config {
            VirtualMachineConfig::RawConfig(config) => (false, BorrowedOrOwned::Borrowed(config)),
            VirtualMachineConfig::AppConfig(config) => {
                let config = load_app_config(config, &debug_config, &temporary_directory)
                    .or_service_specific_exception_with(-1, |e| {
                        *is_protected = config.protectedVm;
                        let message = format!("Failed to load app config: {:?}", e);
                        error!("{}", message);
                        message
                    })?;
                (true, BorrowedOrOwned::Owned(config))
            }
        };
        let config = config.as_ref();
        *is_protected = config.protectedVm;

        // Check if partition images are labeled incorrectly. This is to prevent random images
        // which are not protected by the Android Verified Boot (e.g. bits downloaded by apps) from
        // being loaded in a pVM. This applies to everything but the instance image in the raw
        // config, and everything but the non-executable, generated partitions in the app
        // config.
        config
            .disks
            .iter()
            .flat_map(|disk| disk.partitions.iter())
            .filter(|partition| {
                if is_app_config {
                    !is_safe_app_partition(&partition.label)
                } else {
                    !is_safe_raw_partition(&partition.label)
                }
            })
            .try_for_each(check_label_for_partition)
            .or_service_specific_exception(-1)?;

        // Check if files for payloads and bases are NOT coming from /vendor and /odm, as they may
        // have unstable interfaces.
        // TODO(b/316431494): remove once Treble interfaces are stabilized.
        check_partitions_for_files(config).or_service_specific_exception(-1)?;

        let kernel = maybe_clone_file(&config.kernel)?;
        let initrd = maybe_clone_file(&config.initrd)?;

        if config.protectedVm {
            // In a protected VM, we require custom kernels to come from a trusted source
            // (b/237054515).
            check_label_for_kernel_files(&kernel, &initrd).or_service_specific_exception(-1)?;
            // Fail fast with a meaningful error message in case device doesn't support pVMs.
            check_protected_vm_is_supported()?;
        }

        let zero_filler_path = temporary_directory.join("zero.img");
        write_zero_filler(&zero_filler_path)
            .context("Failed to make composite image")
            .with_log()
            .or_service_specific_exception(-1)?;

        // Assemble disk images if needed.
        let disks = config
            .disks
            .iter()
            .map(|disk| {
                assemble_disk_image(
                    disk,
                    &zero_filler_path,
                    &temporary_directory,
                    &mut next_temporary_image_id,
                    &mut indirect_files,
                )
            })
            .collect::<Result<Vec<DiskFile>, _>>()?;

        let (cpus, host_cpu_topology) = match config.cpuTopology {
            CpuTopology::MATCH_HOST => (None, true),
            CpuTopology::ONE_CPU => (NonZeroU32::new(1), false),
            val => {
                return Err(anyhow!("Failed to parse CPU topology value {:?}", val))
                    .with_log()
                    .or_service_specific_exception(-1);
            }
        };

        let (vfio_devices, dtbo) = if !config.devices.is_empty() {
            let mut set = HashSet::new();
            for device in config.devices.iter() {
                let path = canonicalize(device)
                    .with_context(|| format!("can't canonicalize {device}"))
                    .or_service_specific_exception(-1)?;
                if !set.insert(path) {
                    return Err(anyhow!("duplicated device {device}"))
                        .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
                }
            }
            let devices = GLOBAL_SERVICE.bindDevicesToVfioDriver(&config.devices)?;
            let dtbo_file = File::from(
                GLOBAL_SERVICE
                    .getDtboFile()?
                    .as_ref()
                    .try_clone()
                    .context("Failed to create VM DTBO from ParcelFileDescriptor")
                    .or_binder_exception(ExceptionCode::BAD_PARCELABLE)?,
            );
            (devices, Some(dtbo_file))
        } else {
            (vec![], None)
        };
        let display_config = if cfg!(paravirtualized_devices) {
            config
                .displayConfig
                .as_ref()
                .map(DisplayConfig::new)
                .transpose()
                .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT)?
        } else {
            None
        };
        let gpu_config = if cfg!(paravirtualized_devices) {
            config
                .gpuConfig
                .as_ref()
                .map(GpuConfig::new)
                .transpose()
                .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT)?
        } else {
            None
        };

        let input_device_options = if cfg!(paravirtualized_devices) {
            config
                .inputDevices
                .iter()
                .map(to_input_device_option_from)
                .collect::<Result<Vec<InputDeviceOption>, _>>()
                .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT)?
        } else {
            vec![]
        };

        // Create TAP network interface if the VM supports network.
        let tap = if cfg!(network) && config.networkSupported {
            if *is_protected {
                return Err(anyhow!("Network feature is not supported for pVM yet"))
                    .with_log()
                    .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)?;
            }
            Some(File::from(
                GLOBAL_SERVICE
                    .createTapInterface(&get_this_pid().to_string())?
                    .as_ref()
                    .try_clone()
                    .context("Failed to get TAP interface from ParcelFileDescriptor")
                    .or_binder_exception(ExceptionCode::BAD_PARCELABLE)?,
            ))
        } else {
            None
        };

        let audio_config = if cfg!(paravirtualized_devices) {
            config.audioConfig.as_ref().map(AudioConfig::new)
        } else {
            None
        };

        let usb_config = config
            .usbConfig
            .as_ref()
            .map(UsbConfig::new)
            .unwrap_or(Ok(UsbConfig { controller: false }))
            .or_binder_exception(ExceptionCode::BAD_PARCELABLE)?;

        // Actually start the VM.
        let crosvm_config = CrosvmConfig {
            cid,
            name: config.name.clone(),
            bootloader: maybe_clone_file(&config.bootloader)?,
            kernel,
            initrd,
            disks,
            params: config.params.to_owned(),
            protected: *is_protected,
            debug_config,
            memory_mib: config
                .memoryMib
                .try_into()
                .ok()
                .and_then(NonZeroU32::new)
                .unwrap_or(NonZeroU32::new(256).unwrap()),
            cpus,
            host_cpu_topology,
            console_out_fd,
            console_in_fd,
            log_fd,
            ramdump,
            indirect_files,
            platform_version: parse_platform_version_req(&config.platformVersion)?,
            detect_hangup: is_app_config,
            gdb_port,
            vfio_devices,
            dtbo,
            device_tree_overlay,
            display_config,
            input_device_options,
            hugepages: config.hugePages,
            tap,
            console_input_device: config.consoleInputDevice.clone(),
            boost_uclamp: config.boostUclamp,
            gpu_config,
            audio_config,
            no_balloon: config.noBalloon,
            usb_config,
        };
        let instance = Arc::new(
            VmInstance::new(
                crosvm_config,
                temporary_directory,
                requester_uid,
                requester_debug_pid,
                vm_context,
            )
            .with_context(|| format!("Failed to create VM with config {:?}", config))
            .with_log()
            .or_service_specific_exception(-1)?,
        );
        state.add_vm(Arc::downgrade(&instance));
        Ok(VirtualMachine::create(instance))
    }
}

/// Returns whether a VM config represents a "custom" virtual machine, which requires the
/// USE_CUSTOM_VIRTUAL_MACHINE.
fn is_custom_config(config: &VirtualMachineConfig) -> bool {
    match config {
        // Any raw (non-Microdroid) VM is considered custom.
        VirtualMachineConfig::RawConfig(_) => true,
        VirtualMachineConfig::AppConfig(config) => {
            // Some features are reserved for platform apps only, even when using
            // VirtualMachineAppConfig. Almost all of these features are grouped in the
            // CustomConfig struct:
            // - controlling CPUs;
            // - gdbPort is set, meaning that crosvm will start a gdb server;
            // - using anything other than the default kernel;
            // - specifying devices to be assigned.
            if config.customConfig.is_some() {
                true
            } else {
                // Additional custom features not included in CustomConfig:
                // - specifying a config file;
                // - specifying extra APKs;
                // - specifying an OS other than Microdroid.
                (match &config.payload {
                    Payload::ConfigPath(_) => true,
                    Payload::PayloadConfig(payload_config) => !payload_config.extraApks.is_empty(),
                }) || config.osName != MICRODROID_OS_NAME
            }
        }
    }
}

fn extract_vendor_hashtree_digest(config: &VirtualMachineConfig) -> Result<Option<Vec<u8>>> {
    let VirtualMachineConfig::AppConfig(config) = config else {
        return Ok(None);
    };
    let Some(custom_config) = &config.customConfig else {
        return Ok(None);
    };
    let Some(file) = custom_config.vendorImage.as_ref() else {
        return Ok(None);
    };

    let file = clone_file(file)?;
    let size =
        file.metadata().context("Failed to get metadata from microdroid vendor image")?.len();
    let vbmeta = VbMetaImage::verify_reader_region(&file, 0, size)
        .context("Failed to get vbmeta from microdroid-vendor.img")?;

    for descriptor in vbmeta.descriptors()?.iter() {
        if let vbmeta::Descriptor::Hashtree(_) = descriptor {
            let root_digest = hex::encode(descriptor.to_hashtree()?.root_digest());
            return Ok(Some(root_digest.as_bytes().to_vec()));
        }
    }
    Err(anyhow!("No hashtree digest is extracted from microdroid vendor image"))
}

fn maybe_create_device_tree_overlay(
    config: &VirtualMachineConfig,
    temporary_directory: &Path,
) -> binder::Result<Option<File>> {
    // Currently, VirtMgr adds the host copy of reference DT & untrusted properties
    // (e.g. instance-id)
    let host_ref_dt = Path::new(VM_REFERENCE_DT_ON_HOST_PATH);
    let host_ref_dt = if host_ref_dt.exists()
        && read_dir(host_ref_dt).or_service_specific_exception(-1)?.next().is_some()
    {
        Some(host_ref_dt)
    } else {
        warn!("VM reference DT doesn't exist in host DT");
        None
    };

    let vendor_hashtree_digest = extract_vendor_hashtree_digest(config)
        .context("Failed to extract vendor hashtree digest")
        .or_service_specific_exception(-1)?;

    let trusted_props = if let Some(ref vendor_hashtree_digest) = vendor_hashtree_digest {
        info!(
            "Passing vendor hashtree digest to pvmfw. This will be rejected if it doesn't \
                match the trusted digest in the pvmfw config, causing the VM to fail to start."
        );
        vec![(cstr!("vendor_hashtree_descriptor_root_digest"), vendor_hashtree_digest.as_slice())]
    } else {
        vec![]
    };

    let instance_id;
    let mut untrusted_props = Vec::with_capacity(2);
    if cfg!(llpvm_changes) {
        instance_id = extract_instance_id(config);
        untrusted_props.push((cstr!("instance-id"), &instance_id[..]));
        let want_updatable = extract_want_updatable(config);
        if want_updatable && is_secretkeeper_supported() {
            // Let guest know that it can defer rollback protection to Secretkeeper by setting
            // an empty property in untrusted node in DT. This enables Updatable VMs.
            untrusted_props.push((cstr!("defer-rollback-protection"), &[]))
        }
    }

    let device_tree_overlay = if host_ref_dt.is_some()
        || !untrusted_props.is_empty()
        || !trusted_props.is_empty()
    {
        let dt_output = temporary_directory.join(VM_DT_OVERLAY_PATH);
        let mut data = [0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let fdt =
            create_device_tree_overlay(&mut data, host_ref_dt, &untrusted_props, &trusted_props)
                .map_err(|e| anyhow!("Failed to create DT overlay, {e:?}"))
                .or_service_specific_exception(-1)?;
        fs::write(&dt_output, fdt.as_slice()).or_service_specific_exception(-1)?;
        Some(File::open(dt_output).or_service_specific_exception(-1)?)
    } else {
        None
    };
    Ok(device_tree_overlay)
}

fn write_zero_filler(zero_filler_path: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(zero_filler_path)
        .with_context(|| "Failed to create zero.img")?;
    file.set_len(ZERO_FILLER_SIZE)?;
    Ok(())
}

fn format_as_android_vm_instance(part: &mut dyn Write) -> std::io::Result<()> {
    part.write_all(ANDROID_VM_INSTANCE_MAGIC.as_bytes())?;
    part.write_all(&ANDROID_VM_INSTANCE_VERSION.to_le_bytes())?;
    part.flush()
}

fn format_as_encryptedstore(part: &mut dyn Write) -> std::io::Result<()> {
    part.write_all(UNFORMATTED_STORAGE_MAGIC.as_bytes())?;
    part.flush()
}

fn round_up(input: u64, granularity: u64) -> u64 {
    if granularity == 0 {
        return input;
    }
    // If the input is absurdly large we round down instead of up; it's going to fail anyway.
    let result = input.checked_add(granularity - 1).unwrap_or(input);
    (result / granularity) * granularity
}

fn to_input_device_option_from(input_device: &InputDevice) -> Result<InputDeviceOption> {
    Ok(match input_device {
        InputDevice::SingleTouch(single_touch) => InputDeviceOption::SingleTouch {
            file: clone_file(single_touch.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?)?,
            height: u32::try_from(single_touch.height)?,
            width: u32::try_from(single_touch.width)?,
            name: if !single_touch.name.is_empty() {
                Some(single_touch.name.clone())
            } else {
                None
            },
        },
        InputDevice::EvDev(evdev) => InputDeviceOption::EvDev(clone_file(
            evdev.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?,
        )?),
        InputDevice::Keyboard(keyboard) => InputDeviceOption::Keyboard(clone_file(
            keyboard.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?,
        )?),
        InputDevice::Mouse(mouse) => InputDeviceOption::Mouse(clone_file(
            mouse.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?,
        )?),
        InputDevice::Switches(switches) => InputDeviceOption::Switches(clone_file(
            switches.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?,
        )?),
        InputDevice::Trackpad(trackpad) => InputDeviceOption::MultiTouchTrackpad {
            file: clone_file(trackpad.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?)?,
            height: u32::try_from(trackpad.height)?,
            width: u32::try_from(trackpad.width)?,
            name: if !trackpad.name.is_empty() { Some(trackpad.name.clone()) } else { None },
        },
        InputDevice::MultiTouch(multi_touch) => InputDeviceOption::MultiTouch {
            file: clone_file(multi_touch.pfd.as_ref().ok_or(anyhow!("pfd should have value"))?)?,
            height: u32::try_from(multi_touch.height)?,
            width: u32::try_from(multi_touch.width)?,
            name: if !multi_touch.name.is_empty() { Some(multi_touch.name.clone()) } else { None },
        },
    })
}
/// Given the configuration for a disk image, assembles the `DiskFile` to pass to crosvm.
///
/// This may involve assembling a composite disk from a set of partition images.
fn assemble_disk_image(
    disk: &DiskImage,
    zero_filler_path: &Path,
    temporary_directory: &Path,
    next_temporary_image_id: &mut u64,
    indirect_files: &mut Vec<File>,
) -> Result<DiskFile, Status> {
    let image = if !disk.partitions.is_empty() {
        if disk.image.is_some() {
            warn!("DiskImage {:?} contains both image and partitions.", disk);
            return Err(anyhow!("DiskImage contains both image and partitions"))
                .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
        }

        let composite_image_filenames =
            make_composite_image_filenames(temporary_directory, next_temporary_image_id);
        let (image, partition_files) = make_composite_image(
            &disk.partitions,
            zero_filler_path,
            &composite_image_filenames.composite,
            &composite_image_filenames.header,
            &composite_image_filenames.footer,
        )
        .with_context(|| format!("Failed to make composite disk image with config {:?}", disk))
        .with_log()
        .or_service_specific_exception(-1)?;

        // Pass the file descriptors for the various partition files to crosvm when it
        // is run.
        indirect_files.extend(partition_files);

        image
    } else if let Some(image) = &disk.image {
        clone_file(image)?
    } else {
        warn!("DiskImage {:?} didn't contain image or partitions.", disk);
        return Err(anyhow!("DiskImage didn't contain image or partitions."))
            .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
    };

    Ok(DiskFile { image, writable: disk.writable })
}

fn append_kernel_param(param: &str, vm_config: &mut VirtualMachineRawConfig) {
    if let Some(ref mut params) = vm_config.params {
        params.push(' ');
        params.push_str(param)
    } else {
        vm_config.params = Some(param.to_owned())
    }
}

fn extract_os_name_from_config_path(config: &Path) -> Option<String> {
    if config.extension()?.to_str()? != "json" {
        return None;
    }

    Some(config.with_extension("").file_name()?.to_str()?.to_owned())
}

fn extract_os_names_from_configs(config_glob_pattern: &str) -> Result<HashSet<String>> {
    let configs = glob(config_glob_pattern)?.collect::<Result<Vec<_>, _>>()?;
    let os_names =
        configs.iter().filter_map(|x| extract_os_name_from_config_path(x)).collect::<HashSet<_>>();

    Ok(os_names)
}

fn get_supported_os_names() -> Result<HashSet<String>> {
    if !cfg!(vendor_modules) {
        return Ok(iter::once(MICRODROID_OS_NAME.to_owned()).collect());
    }

    extract_os_names_from_configs("/apex/com.android.virt/etc/microdroid*.json")
}

fn is_valid_os(os_name: &str) -> bool {
    SUPPORTED_OS_NAMES.contains(os_name)
}

fn uses_gki_kernel(config: &VirtualMachineConfig) -> bool {
    if !cfg!(vendor_modules) {
        return false;
    }
    match config {
        VirtualMachineConfig::RawConfig(_) => false,
        VirtualMachineConfig::AppConfig(config) => config.osName.starts_with("microdroid_gki-"),
    }
}

fn load_app_config(
    config: &VirtualMachineAppConfig,
    debug_config: &DebugConfig,
    temporary_directory: &Path,
) -> Result<VirtualMachineRawConfig> {
    let apk_file = clone_file(config.apk.as_ref().unwrap())?;
    let idsig_file = clone_file(config.idsig.as_ref().unwrap())?;
    let instance_file = clone_file(config.instanceImage.as_ref().unwrap())?;

    let storage_image = if let Some(file) = config.encryptedStorageImage.as_ref() {
        Some(clone_file(file)?)
    } else {
        None
    };

    let vm_payload_config;
    let extra_apk_files: Vec<_>;
    match &config.payload {
        Payload::ConfigPath(config_path) => {
            vm_payload_config =
                load_vm_payload_config_from_file(&apk_file, config_path.as_str())
                    .with_context(|| format!("Couldn't read config from {}", config_path))?;
            extra_apk_files = vm_payload_config
                .extra_apks
                .iter()
                .enumerate()
                .map(|(i, apk)| {
                    File::open(PathBuf::from(&apk.path))
                        .with_context(|| format!("Failed to open extra apk #{i} {}", apk.path))
                })
                .collect::<Result<_>>()?;
        }
        Payload::PayloadConfig(payload_config) => {
            vm_payload_config = create_vm_payload_config(payload_config)?;
            extra_apk_files =
                payload_config.extraApks.iter().map(clone_file).collect::<binder::Result<_>>()?;
        }
    };

    let payload_config_os = vm_payload_config.os.name.as_str();
    if !payload_config_os.is_empty() && payload_config_os != "microdroid" {
        bail!("'os' in payload config is deprecated");
    }

    // For now, the only supported OS is Microdroid and Microdroid GKI
    let os_name = config.osName.as_str();
    if !is_valid_os(os_name) {
        bail!("Unknown OS \"{}\"", os_name);
    }

    // It is safe to construct a filename based on the os_name because we've already checked that it
    // is one of the allowed values.
    let vm_config_path = PathBuf::from(format!("/apex/com.android.virt/etc/{}.json", os_name));
    let vm_config_file = File::open(vm_config_path)?;
    let mut vm_config = VmConfig::load(&vm_config_file)?.to_parcelable()?;

    if let Some(custom_config) = &config.customConfig {
        if let Some(file) = custom_config.customKernelImage.as_ref() {
            vm_config.kernel = Some(ParcelFileDescriptor::new(clone_file(file)?))
        }
        vm_config.gdbPort = custom_config.gdbPort;

        if let Some(file) = custom_config.vendorImage.as_ref() {
            add_microdroid_vendor_image(clone_file(file)?, &mut vm_config);
            append_kernel_param("androidboot.microdroid.mount_vendor=1", &mut vm_config)
        }

        vm_config.devices.clone_from(&custom_config.devices);
        vm_config.networkSupported = custom_config.networkSupported;

        for param in custom_config.extraKernelCmdlineParams.iter() {
            append_kernel_param(param, &mut vm_config);
        }
    }

    if config.memoryMib > 0 {
        vm_config.memoryMib = config.memoryMib;
    }

    vm_config.name.clone_from(&config.name);
    vm_config.protectedVm = config.protectedVm;
    vm_config.cpuTopology = config.cpuTopology;
    vm_config.hugePages = config.hugePages || vm_payload_config.hugepages;
    vm_config.boostUclamp = config.boostUclamp;

    // Microdroid takes additional init ramdisk & (optionally) storage image
    add_microdroid_system_images(config, instance_file, storage_image, os_name, &mut vm_config)?;

    // Include Microdroid payload disk (contains apks, idsigs) in vm config
    add_microdroid_payload_images(
        config,
        debug_config,
        temporary_directory,
        apk_file,
        idsig_file,
        extra_apk_files,
        &vm_payload_config,
        &mut vm_config,
    )?;

    Ok(vm_config)
}

fn check_partition_for_file(fd: &ParcelFileDescriptor) -> Result<()> {
    let path = format!("/proc/self/fd/{}", fd.as_raw_fd());
    let link = fs::read_link(&path).context(format!("can't read_link {path}"))?;

    // microdroid vendor image is OK
    if cfg!(vendor_modules) && link == Path::new("/vendor/etc/avf/microdroid/microdroid_vendor.img")
    {
        return Ok(());
    }

    if link.starts_with("/vendor") || link.starts_with("/odm") {
        bail!("vendor or odm file {} can't be used for VM", link.display());
    }

    Ok(())
}

fn check_partitions_for_files(config: &VirtualMachineRawConfig) -> Result<()> {
    config
        .disks
        .iter()
        .flat_map(|disk| disk.partitions.iter())
        .filter_map(|partition| partition.image.as_ref())
        .try_for_each(check_partition_for_file)?;

    config.kernel.as_ref().map_or(Ok(()), check_partition_for_file)?;
    config.initrd.as_ref().map_or(Ok(()), check_partition_for_file)?;
    config.bootloader.as_ref().map_or(Ok(()), check_partition_for_file)?;

    Ok(())
}

fn load_vm_payload_config_from_file(apk_file: &File, config_path: &str) -> Result<VmPayloadConfig> {
    let mut apk_zip = ZipArchive::new(apk_file)?;
    let config_file = apk_zip.by_name(config_path)?;
    Ok(serde_json::from_reader(config_file)?)
}

fn create_vm_payload_config(
    payload_config: &VirtualMachinePayloadConfig,
) -> Result<VmPayloadConfig> {
    // There isn't an actual config file. Construct a synthetic VmPayloadConfig from the explicit
    // parameters we've been given. Microdroid will do something equivalent inside the VM using the
    // payload config that we send it via the metadata file.

    let payload_binary_name = &payload_config.payloadBinaryName;
    if payload_binary_name.contains('/') {
        bail!("Payload binary name must not specify a path: {payload_binary_name}");
    }

    let task = Task { type_: TaskType::MicrodroidLauncher, command: payload_binary_name.clone() };

    // The VM only cares about how many there are, these names are actually ignored.
    let extra_apk_count = payload_config.extraApks.len();
    let extra_apks =
        (0..extra_apk_count).map(|i| ApkConfig { path: format!("extra-apk-{i}") }).collect();

    Ok(VmPayloadConfig { task: Some(task), extra_apks, ..Default::default() })
}

/// Generates a unique filename to use for a composite disk image.
fn make_composite_image_filenames(
    temporary_directory: &Path,
    next_temporary_image_id: &mut u64,
) -> CompositeImageFilenames {
    let id = *next_temporary_image_id;
    *next_temporary_image_id += 1;
    CompositeImageFilenames {
        composite: temporary_directory.join(format!("composite-{}.img", id)),
        header: temporary_directory.join(format!("composite-{}-header.img", id)),
        footer: temporary_directory.join(format!("composite-{}-footer.img", id)),
    }
}

/// Filenames for a composite disk image, including header and footer partitions.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CompositeImageFilenames {
    /// The composite disk image itself.
    composite: PathBuf,
    /// The header partition image.
    header: PathBuf,
    /// The footer partition image.
    footer: PathBuf,
}

/// Checks whether the caller has a specific permission
fn check_permission(perm: &str) -> binder::Result<()> {
    if cfg!(early) {
        // Skip permission check for early VMs, in favor of SELinux
        return Ok(());
    }
    let calling_pid = get_calling_pid();
    let calling_uid = get_calling_uid();
    // Root can do anything
    if calling_uid == 0 {
        return Ok(());
    }
    let perm_svc: Strong<dyn IPermissionController::IPermissionController> =
        binder::wait_for_interface("permission")?;
    if perm_svc.checkPermission(perm, calling_pid, calling_uid as i32)? {
        Ok(())
    } else {
        Err(anyhow!("does not have the {} permission", perm))
            .or_binder_exception(ExceptionCode::SECURITY)
    }
}

/// Check whether the caller of the current Binder method is allowed to manage VMs
fn check_manage_access() -> binder::Result<()> {
    check_permission("android.permission.MANAGE_VIRTUAL_MACHINE")
}

/// Check whether the caller of the current Binder method is allowed to create custom VMs
fn check_use_custom_virtual_machine() -> binder::Result<()> {
    check_permission("android.permission.USE_CUSTOM_VIRTUAL_MACHINE")
}

/// Return whether a partition is exempt from selinux label checks, because we know that it does
/// not contain code and is likely to be generated in an app-writable directory.
fn is_safe_app_partition(label: &str) -> bool {
    // See add_microdroid_system_images & add_microdroid_payload_images in payload.rs.
    label == "vm-instance"
        || label == "encryptedstore"
        || label == "microdroid-apk-idsig"
        || label == "payload-metadata"
        || label.starts_with("extra-idsig-")
}

/// Returns whether a partition with the given label is safe for a raw config VM.
fn is_safe_raw_partition(label: &str) -> bool {
    label == "vm-instance"
}

/// Check that a file SELinux label is acceptable.
///
/// We only want to allow code in a VM to be sourced from places that apps, and the
/// system or vendor, do not have write access to.
///
/// Note that sepolicy must also grant read access for these types to both virtualization
/// service and crosvm.
///
/// App private data files are deliberately excluded, to avoid arbitrary payloads being run on
/// user devices (W^X).
fn check_label_is_allowed(context: &SeContext) -> Result<()> {
    match context.selinux_type()? {
        | "apk_data_file" // APKs of an installed app
        | "shell_data_file" // test files created via adb shell
        | "staging_data_file" // updated/staged APEX images
        | "system_file" // immutable dm-verity protected partition
        | "virtualizationservice_data_file" // files created by VS / VirtMgr
        | "vendor_microdroid_file" // immutable dm-verity protected partition (/vendor/etc/avf/microdroid/.*)
         => Ok(()),
        _ => bail!("Label {} is not allowed", context),
    }
}

fn check_label_for_partition(partition: &Partition) -> Result<()> {
    let file = partition.image.as_ref().unwrap().as_ref();
    check_label_is_allowed(&getfilecon(file)?)
        .with_context(|| format!("Partition {} invalid", &partition.label))
}

fn check_label_for_kernel_files(kernel: &Option<File>, initrd: &Option<File>) -> Result<()> {
    if let Some(f) = kernel {
        check_label_for_file(f, "kernel")?;
    }
    if let Some(f) = initrd {
        check_label_for_file(f, "initrd")?;
    }
    Ok(())
}
fn check_label_for_file(file: &File, name: &str) -> Result<()> {
    check_label_is_allowed(&getfilecon(file)?).with_context(|| format!("{} file invalid", name))
}

/// Implementation of the AIDL `IVirtualMachine` interface. Used as a handle to a VM.
#[derive(Debug)]
struct VirtualMachine {
    instance: Arc<VmInstance>,
}

impl VirtualMachine {
    fn create(instance: Arc<VmInstance>) -> Strong<dyn IVirtualMachine> {
        BnVirtualMachine::new_binder(VirtualMachine { instance }, BinderFeatures::default())
    }
}

impl Interface for VirtualMachine {}

impl IVirtualMachine for VirtualMachine {
    fn getCid(&self) -> binder::Result<i32> {
        // Don't check permission. The owner of the VM might have passed this binder object to
        // others.
        Ok(self.instance.cid as i32)
    }

    fn getState(&self) -> binder::Result<VirtualMachineState> {
        // Don't check permission. The owner of the VM might have passed this binder object to
        // others.
        Ok(get_state(&self.instance))
    }

    fn registerCallback(
        &self,
        callback: &Strong<dyn IVirtualMachineCallback>,
    ) -> binder::Result<()> {
        // Don't check permission. The owner of the VM might have passed this binder object to
        // others.
        //
        // TODO: Should this give an error if the VM is already dead?
        self.instance.callbacks.add(callback.clone());
        Ok(())
    }

    fn start(&self) -> binder::Result<()> {
        self.instance
            .start()
            .with_context(|| format!("Error starting VM with CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)
    }

    fn stop(&self) -> binder::Result<()> {
        self.instance
            .kill()
            .with_context(|| format!("Error stopping VM with CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)
    }

    fn getMemoryBalloon(&self) -> binder::Result<i64> {
        let balloon = self
            .instance
            .get_memory_balloon()
            .with_context(|| format!("Error getting balloon for VM with CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)?;
        Ok(balloon.try_into().unwrap())
    }

    fn setMemoryBalloon(&self, num_bytes: i64) -> binder::Result<()> {
        self.instance
            .set_memory_balloon(num_bytes.try_into().unwrap())
            .with_context(|| format!("Error setting balloon for VM with CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)
    }

    fn connectVsock(&self, port: i32) -> binder::Result<ParcelFileDescriptor> {
        if !matches!(&*self.instance.vm_state.lock().unwrap(), VmState::Running { .. }) {
            return Err(anyhow!("VM is not running")).or_service_specific_exception(-1);
        }
        let port = port as u32;
        if port < 1024 {
            return Err(anyhow!("Can't connect to privileged port {port}"))
                .or_service_specific_exception(-1);
        }
        let stream = VsockStream::connect_with_cid_port(self.instance.cid, port)
            .context("Failed to connect")
            .or_service_specific_exception(-1)?;
        Ok(vsock_stream_to_pfd(stream))
    }

    fn setHostConsoleName(&self, ptsname: &str) -> binder::Result<()> {
        self.instance.vm_context.global_context.setHostConsoleName(ptsname)
    }

    fn suspend(&self) -> binder::Result<()> {
        self.instance
            .suspend()
            .with_context(|| format!("Error suspending VM with CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)
    }

    fn resume(&self) -> binder::Result<()> {
        self.instance
            .resume()
            .with_context(|| format!("Error resuming VM with CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)
    }
}

impl Drop for VirtualMachine {
    fn drop(&mut self) {
        debug!("Dropping {:?}", self);
        if let Err(e) = self.instance.kill() {
            debug!("Error stopping dropped VM with CID {}: {:?}", self.instance.cid, e);
        }
    }
}

/// A set of Binders to be called back in response to various events on the VM, such as when it
/// dies.
#[derive(Debug, Default)]
pub struct VirtualMachineCallbacks(Mutex<Vec<Strong<dyn IVirtualMachineCallback>>>);

impl VirtualMachineCallbacks {
    /// Call all registered callbacks to notify that the payload has started.
    pub fn notify_payload_started(&self, cid: Cid) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onPayloadStarted(cid as i32) {
                error!("Error notifying payload start event from VM CID {}: {:?}", cid, e);
            }
        }
    }

    /// Call all registered callbacks to notify that the payload is ready to serve.
    pub fn notify_payload_ready(&self, cid: Cid) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onPayloadReady(cid as i32) {
                error!("Error notifying payload ready event from VM CID {}: {:?}", cid, e);
            }
        }
    }

    /// Call all registered callbacks to notify that the payload has finished.
    pub fn notify_payload_finished(&self, cid: Cid, exit_code: i32) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onPayloadFinished(cid as i32, exit_code) {
                error!("Error notifying payload finish event from VM CID {}: {:?}", cid, e);
            }
        }
    }

    /// Call all registered callbacks to say that the VM encountered an error.
    pub fn notify_error(&self, cid: Cid, error_code: ErrorCode, message: &str) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onError(cid as i32, error_code, message) {
                error!("Error notifying error event from VM CID {}: {:?}", cid, e);
            }
        }
    }

    /// Call all registered callbacks to say that the VM has died.
    pub fn callback_on_died(&self, cid: Cid, reason: DeathReason) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onDied(cid as i32, reason) {
                error!("Error notifying exit of VM CID {}: {:?}", cid, e);
            }
        }
    }

    /// Add a new callback to the set.
    fn add(&self, callback: Strong<dyn IVirtualMachineCallback>) {
        self.0.lock().unwrap().push(callback);
    }
}

/// The mutable state of the VirtualizationService. There should only be one instance of this
/// struct.
#[derive(Debug, Default)]
struct State {
    /// The VMs which have been started. When VMs are started a weak reference is added to this
    /// list while a strong reference is returned to the caller over Binder. Once all copies of
    /// the Binder client are dropped the weak reference here will become invalid, and will be
    /// removed from the list opportunistically the next time `add_vm` is called.
    vms: Vec<Weak<VmInstance>>,
}

impl State {
    /// Get a list of VMs which still have Binder references to them.
    fn vms(&self) -> Vec<Arc<VmInstance>> {
        // Attempt to upgrade the weak pointers to strong pointers.
        self.vms.iter().filter_map(Weak::upgrade).collect()
    }

    /// Add a new VM to the list.
    fn add_vm(&mut self, vm: Weak<VmInstance>) {
        // Garbage collect any entries from the stored list which no longer exist.
        self.vms.retain(|vm| vm.strong_count() > 0);

        // Actually add the new VM.
        self.vms.push(vm);
    }

    /// Get a VM that corresponds to the given cid
    fn get_vm(&self, cid: Cid) -> Option<Arc<VmInstance>> {
        self.vms().into_iter().find(|vm| vm.cid == cid)
    }
}

/// Gets the `VirtualMachineState` of the given `VmInstance`.
fn get_state(instance: &VmInstance) -> VirtualMachineState {
    match &*instance.vm_state.lock().unwrap() {
        VmState::NotStarted { .. } => VirtualMachineState::NOT_STARTED,
        VmState::Running { .. } => match instance.payload_state() {
            PayloadState::Starting => VirtualMachineState::STARTING,
            PayloadState::Started => VirtualMachineState::STARTED,
            PayloadState::Ready => VirtualMachineState::READY,
            PayloadState::Finished => VirtualMachineState::FINISHED,
            PayloadState::Hangup => VirtualMachineState::DEAD,
        },
        VmState::Dead => VirtualMachineState::DEAD,
        VmState::Failed => VirtualMachineState::DEAD,
    }
}

/// Converts a `&ParcelFileDescriptor` to a `File` by cloning the file.
pub fn clone_file(file: &ParcelFileDescriptor) -> binder::Result<File> {
    file.as_ref()
        .try_clone()
        .context("Failed to clone File from ParcelFileDescriptor")
        .or_binder_exception(ExceptionCode::BAD_PARCELABLE)
        .map(File::from)
}

/// Converts an `&Option<ParcelFileDescriptor>` to an `Option<File>` by cloning the file.
fn maybe_clone_file(file: &Option<ParcelFileDescriptor>) -> binder::Result<Option<File>> {
    file.as_ref().map(clone_file).transpose()
}

/// Converts a `VsockStream` to a `ParcelFileDescriptor`.
fn vsock_stream_to_pfd(stream: VsockStream) -> ParcelFileDescriptor {
    // SAFETY: ownership is transferred from stream to f
    let f = unsafe { File::from_raw_fd(stream.into_raw_fd()) };
    ParcelFileDescriptor::new(f)
}

/// Parses the platform version requirement string.
fn parse_platform_version_req(s: &str) -> binder::Result<VersionReq> {
    VersionReq::parse(s)
        .with_context(|| format!("Invalid platform version requirement {}", s))
        .or_binder_exception(ExceptionCode::BAD_PARCELABLE)
}

/// Create the empty ramdump file
fn prepare_ramdump_file(temporary_directory: &Path) -> binder::Result<File> {
    // `ramdump_write` is sent to crosvm and will be the backing store for the /dev/hvc1 where
    // VM will emit ramdump to. `ramdump_read` will be sent back to the client (i.e. the VM
    // owner) for readout.
    let ramdump_path = temporary_directory.join("ramdump");
    let ramdump = File::create(ramdump_path)
        .context("Failed to prepare ramdump file")
        .with_log()
        .or_service_specific_exception(-1)?;
    Ok(ramdump)
}

fn is_protected(config: &VirtualMachineConfig) -> bool {
    match config {
        VirtualMachineConfig::RawConfig(config) => config.protectedVm,
        VirtualMachineConfig::AppConfig(config) => config.protectedVm,
    }
}

fn check_gdb_allowed(config: &VirtualMachineConfig) -> binder::Result<()> {
    if is_protected(config) {
        return Err(anyhow!("Can't use gdb with protected VMs"))
            .or_binder_exception(ExceptionCode::SECURITY);
    }

    if get_debug_level(config) == Some(DebugLevel::NONE) {
        return Err(anyhow!("Can't use gdb with non-debuggable VMs"))
            .or_binder_exception(ExceptionCode::SECURITY);
    }

    Ok(())
}

fn extract_instance_id(config: &VirtualMachineConfig) -> [u8; 64] {
    match config {
        VirtualMachineConfig::RawConfig(config) => config.instanceId,
        VirtualMachineConfig::AppConfig(config) => config.instanceId,
    }
}

fn extract_want_updatable(config: &VirtualMachineConfig) -> bool {
    match config {
        VirtualMachineConfig::RawConfig(_) => true,
        VirtualMachineConfig::AppConfig(config) => {
            let Some(custom) = &config.customConfig else { return true };
            custom.wantUpdatable
        }
    }
}

fn extract_gdb_port(config: &VirtualMachineConfig) -> Option<NonZeroU16> {
    match config {
        VirtualMachineConfig::RawConfig(config) => NonZeroU16::new(config.gdbPort as u16),
        VirtualMachineConfig::AppConfig(config) => {
            NonZeroU16::new(config.customConfig.as_ref().map(|c| c.gdbPort).unwrap_or(0) as u16)
        }
    }
}

fn check_no_vendor_modules(config: &VirtualMachineConfig) -> binder::Result<()> {
    let VirtualMachineConfig::AppConfig(config) = config else { return Ok(()) };
    if let Some(custom_config) = &config.customConfig {
        if custom_config.vendorImage.is_some() || custom_config.customKernelImage.is_some() {
            return Err(anyhow!("vendor modules feature is disabled"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }
    }
    Ok(())
}

fn check_no_devices(config: &VirtualMachineConfig) -> binder::Result<()> {
    let VirtualMachineConfig::AppConfig(config) = config else { return Ok(()) };
    if let Some(custom_config) = &config.customConfig {
        if !custom_config.devices.is_empty() {
            return Err(anyhow!("device assignment feature is disabled"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }
    }
    Ok(())
}

fn check_no_extra_apks(config: &VirtualMachineConfig) -> binder::Result<()> {
    let VirtualMachineConfig::AppConfig(config) = config else { return Ok(()) };
    let Payload::PayloadConfig(payload_config) = &config.payload else { return Ok(()) };
    if !payload_config.extraApks.is_empty() {
        return Err(anyhow!("multi-tenant feature is disabled"))
            .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
    }
    Ok(())
}

fn check_no_extra_kernel_cmdline_params(config: &VirtualMachineConfig) -> binder::Result<()> {
    let VirtualMachineConfig::AppConfig(config) = config else { return Ok(()) };
    if let Some(custom_config) = &config.customConfig {
        if !custom_config.extraKernelCmdlineParams.is_empty() {
            return Err(anyhow!("debuggable_vms_improvements feature is disabled"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }
    }
    Ok(())
}

fn check_protected_vm_is_supported() -> binder::Result<()> {
    let is_pvm_supported =
        hypervisor_props::is_protected_vm_supported().or_service_specific_exception(-1)?;
    if is_pvm_supported {
        Ok(())
    } else {
        Err(anyhow!("pVM is not supported"))
            .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }
}

fn check_config_features(config: &VirtualMachineConfig) -> binder::Result<()> {
    if !cfg!(vendor_modules) {
        check_no_vendor_modules(config)?;
    }
    if !cfg!(device_assignment) {
        check_no_devices(config)?;
    }
    if !cfg!(multi_tenant) {
        check_no_extra_apks(config)?;
    }
    if !cfg!(debuggable_vms_improvements) {
        check_no_extra_kernel_cmdline_params(config)?;
    }
    Ok(())
}

fn check_config_allowed_for_early_vms(config: &VirtualMachineConfig) -> binder::Result<()> {
    check_no_vendor_modules(config)?;
    check_no_devices(config)?;

    Ok(())
}

fn clone_or_prepare_logger_fd(
    fd: Option<&ParcelFileDescriptor>,
    tag: String,
) -> Result<Option<File>, Status> {
    if let Some(fd) = fd {
        return Ok(Some(clone_file(fd)?));
    }

    let (read_fd, write_fd) =
        pipe().context("Failed to create pipe").or_service_specific_exception(-1)?;

    let mut reader = BufReader::new(File::from(read_fd));
    let write_fd = File::from(write_fd);

    std::thread::spawn(move || loop {
        let mut buf = vec![];
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => {
                // EOF
                return;
            }
            Ok(size) => {
                if buf[size - 1] == b'\n' {
                    buf.pop();
                }
                info!("{}: {}", &tag, &String::from_utf8_lossy(&buf));
            }
            Err(e) => {
                error!("Could not read console pipe: {:?}", e);
                return;
            }
        };
    });

    Ok(Some(write_fd))
}

/// Simple utility for referencing Borrowed or Owned. Similar to std::borrow::Cow, but
/// it doesn't require that T implements Clone.
enum BorrowedOrOwned<'a, T> {
    Borrowed(&'a T),
    Owned(T),
}

impl<'a, T> AsRef<T> for BorrowedOrOwned<'a, T> {
    fn as_ref(&self) -> &T {
        match self {
            Self::Borrowed(b) => b,
            Self::Owned(o) => o,
        }
    }
}

/// Implementation of `IVirtualMachineService`, the entry point of the AIDL service.
#[derive(Debug, Default)]
struct VirtualMachineService {
    state: Arc<Mutex<State>>,
    cid: Cid,
}

impl Interface for VirtualMachineService {}

impl IVirtualMachineService for VirtualMachineService {
    fn notifyPayloadStarted(&self) -> binder::Result<()> {
        let cid = self.cid;
        if let Some(vm) = self.state.lock().unwrap().get_vm(cid) {
            info!("VM with CID {} started payload", cid);
            vm.update_payload_state(PayloadState::Started)
                .or_binder_exception(ExceptionCode::ILLEGAL_STATE)?;
            vm.callbacks.notify_payload_started(cid);

            let vm_start_timestamp = vm.vm_metric.lock().unwrap().start_timestamp;
            write_vm_booted_stats(vm.requester_uid as i32, &vm.name, vm_start_timestamp);
            Ok(())
        } else {
            error!("notifyPayloadStarted is called from an unknown CID {}", cid);
            Err(anyhow!("cannot find a VM with CID {}", cid)).or_service_specific_exception(-1)
        }
    }

    fn notifyPayloadReady(&self) -> binder::Result<()> {
        let cid = self.cid;
        if let Some(vm) = self.state.lock().unwrap().get_vm(cid) {
            info!("VM with CID {} reported payload is ready", cid);
            vm.update_payload_state(PayloadState::Ready)
                .or_binder_exception(ExceptionCode::ILLEGAL_STATE)?;
            vm.callbacks.notify_payload_ready(cid);
            Ok(())
        } else {
            error!("notifyPayloadReady is called from an unknown CID {}", cid);
            Err(anyhow!("cannot find a VM with CID {}", cid)).or_service_specific_exception(-1)
        }
    }

    fn notifyPayloadFinished(&self, exit_code: i32) -> binder::Result<()> {
        let cid = self.cid;
        if let Some(vm) = self.state.lock().unwrap().get_vm(cid) {
            info!("VM with CID {} finished payload", cid);
            vm.update_payload_state(PayloadState::Finished)
                .or_binder_exception(ExceptionCode::ILLEGAL_STATE)?;
            vm.callbacks.notify_payload_finished(cid, exit_code);
            Ok(())
        } else {
            error!("notifyPayloadFinished is called from an unknown CID {}", cid);
            Err(anyhow!("cannot find a VM with CID {}", cid)).or_service_specific_exception(-1)
        }
    }

    fn notifyError(&self, error_code: ErrorCode, message: &str) -> binder::Result<()> {
        let cid = self.cid;
        if let Some(vm) = self.state.lock().unwrap().get_vm(cid) {
            info!("VM with CID {} encountered an error", cid);
            vm.update_payload_state(PayloadState::Finished)
                .or_binder_exception(ExceptionCode::ILLEGAL_STATE)?;
            vm.callbacks.notify_error(cid, error_code, message);
            Ok(())
        } else {
            error!("notifyError is called from an unknown CID {}", cid);
            Err(anyhow!("cannot find a VM with CID {}", cid)).or_service_specific_exception(-1)
        }
    }

    fn getSecretkeeper(&self) -> binder::Result<Strong<dyn ISecretkeeper>> {
        if !is_secretkeeper_supported() {
            return Err(StatusCode::NAME_NOT_FOUND)?;
        }
        let sk = binder::wait_for_interface(SECRETKEEPER_IDENTIFIER)?;
        Ok(BnSecretkeeper::new_binder(SecretkeeperProxy(sk), BinderFeatures::default()))
    }

    fn requestAttestation(&self, csr: &[u8], test_mode: bool) -> binder::Result<Vec<Certificate>> {
        GLOBAL_SERVICE.requestAttestation(csr, get_calling_uid() as i32, test_mode)
    }
}

fn is_secretkeeper_supported() -> bool {
    binder::is_declared(SECRETKEEPER_IDENTIFIER)
        .expect("Could not check for declared Secretkeeper interface")
}

impl VirtualMachineService {
    fn new_binder(state: Arc<Mutex<State>>, cid: Cid) -> Strong<dyn IVirtualMachineService> {
        BnVirtualMachineService::new_binder(
            VirtualMachineService { state, cid },
            BinderFeatures::default(),
        )
    }
}

struct SecretkeeperProxy(Strong<dyn ISecretkeeper>);

impl Interface for SecretkeeperProxy {}

impl ISecretkeeper for SecretkeeperProxy {
    fn processSecretManagementRequest(&self, req: &[u8]) -> binder::Result<Vec<u8>> {
        // Pass the request to the channel, and read the response.
        self.0.processSecretManagementRequest(req)
    }

    fn getAuthGraphKe(&self) -> binder::Result<Strong<dyn IAuthGraphKeyExchange>> {
        let ag = AuthGraphKeyExchangeProxy(self.0.getAuthGraphKe()?);
        Ok(BnAuthGraphKeyExchange::new_binder(ag, BinderFeatures::default()))
    }

    fn deleteIds(&self, ids: &[SecretId]) -> binder::Result<()> {
        self.0.deleteIds(ids)
    }

    fn deleteAll(&self) -> binder::Result<()> {
        self.0.deleteAll()
    }
}

struct AuthGraphKeyExchangeProxy(Strong<dyn IAuthGraphKeyExchange>);

impl Interface for AuthGraphKeyExchangeProxy {}

impl IAuthGraphKeyExchange for AuthGraphKeyExchangeProxy {
    fn create(&self) -> binder::Result<SessionInitiationInfo> {
        self.0.create()
    }

    fn init(
        &self,
        peer_pub_key: &PubKey,
        peer_id: &Identity,
        peer_nonce: &[u8],
        peer_version: i32,
    ) -> binder::Result<KeInitResult> {
        self.0.init(peer_pub_key, peer_id, peer_nonce, peer_version)
    }

    fn finish(
        &self,
        peer_pub_key: &PubKey,
        peer_id: &Identity,
        peer_signature: &SessionIdSignature,
        peer_nonce: &[u8],
        peer_version: i32,
        own_key: &Key,
    ) -> binder::Result<SessionInfo> {
        self.0.finish(peer_pub_key, peer_id, peer_signature, peer_nonce, peer_version, own_key)
    }

    fn authenticationComplete(
        &self,
        peer_signature: &SessionIdSignature,
        shared_keys: &[AuthgraphArc; 2],
    ) -> binder::Result<[AuthgraphArc; 2]> {
        self.0.authenticationComplete(peer_signature, shared_keys)
    }
}

// KEEP IN SYNC WITH early_vms.xsd
#[derive(Debug, Deserialize, PartialEq)]
struct EarlyVm {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    cid: i32,
    #[allow(dead_code)]
    path: String,
}

#[derive(Debug, Default, Deserialize)]
struct EarlyVms {
    #[allow(dead_code)]
    early_vm: Vec<EarlyVm>,
}

fn range_for_partition(partition: &str) -> Result<Range<Cid>> {
    match partition {
        "system" => Ok(100..200),
        "system_ext" | "product" => Ok(200..300),
        _ => Err(anyhow!("Early VMs are not supported for {partition}")),
    }
}

fn find_early_vm(xml_path: &Path, cid_range: &Range<Cid>, name: &str) -> Result<EarlyVm> {
    if !xml_path.exists() {
        bail!("{} doesn't exist", xml_path.display());
    }

    let xml =
        fs::read(xml_path).with_context(|| format!("Failed to read {}", xml_path.display()))?;
    let xml = String::from_utf8(xml)
        .with_context(|| format!("{} is not a valid UTF-8 file", xml_path.display()))?;
    let early_vms: EarlyVms = serde_xml_rs::from_str(&xml)
        .with_context(|| format!("Can't parse {}", xml_path.display()))?;

    let mut found_vm: Option<EarlyVm> = None;

    for early_vm in early_vms.early_vm {
        if early_vm.name != name {
            continue;
        }

        let cid = early_vm
            .cid
            .try_into()
            .with_context(|| format!("Invalid CID value {}", early_vm.cid))?;

        if !cid_range.contains(&cid) {
            bail!("VM '{}' uses CID {cid} which is out of range. Available CIDs for '{}': {cid_range:?}", xml_path.display(), early_vm.name);
        }

        if found_vm.is_some() {
            bail!("Multiple VMs named {name} are found in {}", xml_path.display());
        }

        found_vm = Some(early_vm);
    }

    found_vm.ok_or_else(|| anyhow!("Can't find {name} in {}", xml_path.display()))
}

fn find_early_vm_for_partition(partition: &str, name: &str) -> Result<EarlyVm> {
    let cid_range = range_for_partition(partition)?;
    find_early_vm(Path::new(&format!("/{partition}/etc/avf/early_vms.xml")), &cid_range, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_allowed_label_for_partition() -> Result<()> {
        let expected_results = vec![
            ("u:object_r:system_file:s0", true),
            ("u:object_r:apk_data_file:s0", true),
            ("u:object_r:app_data_file:s0", false),
            ("u:object_r:app_data_file:s0:c512,c768", false),
            ("u:object_r:privapp_data_file:s0:c512,c768", false),
            ("invalid", false),
            ("user:role:apk_data_file:severity:categories", true),
            ("user:role:apk_data_file:severity:categories:extraneous", false),
        ];

        for (label, expected_valid) in expected_results {
            let context = SeContext::new(label)?;
            let result = check_label_is_allowed(&context);
            if expected_valid {
                assert!(result.is_ok(), "Expected label {} to be allowed, got {:?}", label, result);
            } else if result.is_ok() {
                bail!("Expected label {} to be disallowed", label);
            }
        }
        Ok(())
    }

    #[test]
    fn test_create_or_update_idsig_file_empty_apk() -> Result<()> {
        let apk = tempfile::tempfile().unwrap();
        let idsig = tempfile::tempfile().unwrap();

        let ret = create_or_update_idsig_file(
            &ParcelFileDescriptor::new(apk),
            &ParcelFileDescriptor::new(idsig),
        );
        assert!(ret.is_err(), "should fail");
        Ok(())
    }

    #[test]
    fn test_create_or_update_idsig_dir_instead_of_file_for_apk() -> Result<()> {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let apk = File::open(tmp_dir.path()).unwrap();
        let idsig = tempfile::tempfile().unwrap();

        let ret = create_or_update_idsig_file(
            &ParcelFileDescriptor::new(apk),
            &ParcelFileDescriptor::new(idsig),
        );
        assert!(ret.is_err(), "should fail");
        Ok(())
    }

    /// Verifies that create_or_update_idsig_file won't oom if a fd that corresponds to a directory
    /// on ext4 filesystem is passed.
    /// On ext4 lseek on a directory fd will return (off_t)-1 (see:
    /// https://bugzilla.kernel.org/show_bug.cgi?id=200043), which will result in
    /// create_or_update_idsig_file ooming while attempting to allocate petabytes of memory.
    #[test]
    fn test_create_or_update_idsig_does_not_crash_dir_on_ext4() -> Result<()> {
        // APEXes are backed by the ext4.
        let apk = File::open("/apex/com.android.virt/").unwrap();
        let idsig = tempfile::tempfile().unwrap();

        let ret = create_or_update_idsig_file(
            &ParcelFileDescriptor::new(apk),
            &ParcelFileDescriptor::new(idsig),
        );
        assert!(ret.is_err(), "should fail");
        Ok(())
    }

    #[test]
    fn test_create_or_update_idsig_does_not_update_if_already_valid() -> Result<()> {
        use std::io::Seek;

        // Pick any APK
        let mut apk = File::open("/system/priv-app/Shell/Shell.apk").unwrap();
        let mut idsig = tempfile::tempfile().unwrap();

        create_or_update_idsig_file(
            &ParcelFileDescriptor::new(apk.try_clone()?),
            &ParcelFileDescriptor::new(idsig.try_clone()?),
        )?;
        let modified_orig = idsig.metadata()?.modified()?;
        apk.rewind()?;
        idsig.rewind()?;

        // Call the function again
        create_or_update_idsig_file(
            &ParcelFileDescriptor::new(apk.try_clone()?),
            &ParcelFileDescriptor::new(idsig.try_clone()?),
        )?;
        let modified_new = idsig.metadata()?.modified()?;
        assert!(modified_orig == modified_new, "idsig file was updated unnecessarily");
        Ok(())
    }

    #[test]
    fn test_create_or_update_idsig_on_non_empty_file() -> Result<()> {
        use std::io::Read;

        // Pick any APK
        let mut apk = File::open("/system/priv-app/Shell/Shell.apk").unwrap();
        let idsig_empty = tempfile::tempfile().unwrap();
        let mut idsig_invalid = tempfile::tempfile().unwrap();
        idsig_invalid.write_all(b"Oops")?;

        // Create new idsig
        create_or_update_idsig_file(
            &ParcelFileDescriptor::new(apk.try_clone()?),
            &ParcelFileDescriptor::new(idsig_empty.try_clone()?),
        )?;
        apk.rewind()?;

        // Update idsig_invalid
        create_or_update_idsig_file(
            &ParcelFileDescriptor::new(apk.try_clone()?),
            &ParcelFileDescriptor::new(idsig_invalid.try_clone()?),
        )?;

        // Ensure the 2 idsig files have same size!
        assert!(
            idsig_empty.metadata()?.len() == idsig_invalid.metadata()?.len(),
            "idsig files differ in size"
        );
        // Ensure the 2 idsig files have same content!
        for (b1, b2) in idsig_empty.bytes().zip(idsig_invalid.bytes()) {
            assert!(b1.unwrap() == b2.unwrap(), "idsig files differ")
        }
        Ok(())
    }
    #[test]
    fn test_append_kernel_param_first_param() {
        let mut vm_config = VirtualMachineRawConfig { ..Default::default() };
        append_kernel_param("foo=1", &mut vm_config);
        assert_eq!(vm_config.params, Some("foo=1".to_owned()))
    }

    #[test]
    fn test_append_kernel_param() {
        let mut vm_config =
            VirtualMachineRawConfig { params: Some("foo=5".to_owned()), ..Default::default() };
        append_kernel_param("bar=42", &mut vm_config);
        assert_eq!(vm_config.params, Some("foo=5 bar=42".to_owned()))
    }

    fn test_extract_os_name_from_config_path(
        path: &Path,
        expected_result: Option<&str>,
    ) -> Result<()> {
        let result = extract_os_name_from_config_path(path);
        if result.as_deref() != expected_result {
            bail!("Expected {:?} but was {:?}", expected_result, &result)
        }
        Ok(())
    }

    #[test]
    fn test_extract_os_name_from_microdroid_config() -> Result<()> {
        test_extract_os_name_from_config_path(
            Path::new("/apex/com.android.virt/etc/microdroid.json"),
            Some("microdroid"),
        )
    }

    #[test]
    fn test_extract_os_name_from_microdroid_gki_config() -> Result<()> {
        test_extract_os_name_from_config_path(
            Path::new("/apex/com.android.virt/etc/microdroid_gki-android14-6.1.json"),
            Some("microdroid_gki-android14-6.1"),
        )
    }

    #[test]
    fn test_extract_os_name_from_invalid_path() -> Result<()> {
        test_extract_os_name_from_config_path(
            Path::new("/apex/com.android.virt/etc/microdroid.img"),
            None,
        )
    }

    #[test]
    fn test_extract_os_name_from_configs() -> Result<()> {
        let tmp_dir = tempfile::TempDir::new()?;
        let tmp_dir_path = tmp_dir.path().to_owned();

        let mut os_names: HashSet<String> = HashSet::new();
        os_names.insert("microdroid".to_owned());
        os_names.insert("microdroid_gki-android14-6.1".to_owned());
        os_names.insert("microdroid_gki-android15-6.1".to_owned());

        // config files
        for os_name in &os_names {
            std::fs::write(tmp_dir_path.join(os_name.to_owned() + ".json"), b"")?;
        }

        // fake files not related to configs
        std::fs::write(tmp_dir_path.join("microdroid_super.img"), b"")?;
        std::fs::write(tmp_dir_path.join("microdroid_foobar.apk"), b"")?;

        let glob_pattern = match tmp_dir_path.join("microdroid*.json").to_str() {
            Some(s) => s.to_owned(),
            None => bail!("tmp_dir_path {:?} is not UTF-8", tmp_dir_path),
        };

        let result = extract_os_names_from_configs(&glob_pattern)?;
        if result != os_names {
            bail!("Expected {:?} but was {:?}", os_names, result);
        }
        Ok(())
    }

    #[test]
    fn test_find_early_vms_from_xml() -> Result<()> {
        let tmp_dir = tempfile::TempDir::new()?;
        let tmp_dir_path = tmp_dir.path().to_owned();
        let xml_path = tmp_dir_path.join("early_vms.xml");

        std::fs::write(
            &xml_path,
            br#"<?xml version="1.0" encoding="utf-8"?>
        <early_vms>
            <early_vm>
                <name>vm_demo_native_early</name>
                <cid>123</cid>
                <path>/system/bin/vm_demo_native_early</path>
            </early_vm>
            <early_vm>
                <name>vm_demo_duplicated_name</name>
                <cid>456</cid>
                <path>/system/bin/vm_demo_duplicated_name_1</path>
            </early_vm>
            <early_vm>
                <name>vm_demo_duplicated_name</name>
                <cid>789</cid>
                <path>/system/bin/vm_demo_duplicated_name_2</path>
            </early_vm>
            <early_vm>
                <name>vm_demo_invalid_cid_1</name>
                <cid>-1</cid>
                <path>/system/bin/vm_demo_invalid_cid_1</path>
            </early_vm>
            <early_vm>
                <name>vm_demo_invalid_cid_2</name>
                <cid>999999</cid>
                <path>/system/bin/vm_demo_invalid_cid_2</path>
            </early_vm>
        </early_vms>
        "#,
        )?;

        let cid_range = 100..1000;

        let result = find_early_vm(&xml_path, &cid_range, "vm_demo_native_early")?;
        let expected = EarlyVm {
            name: "vm_demo_native_early".to_owned(),
            cid: 123,
            path: "/system/bin/vm_demo_native_early".to_owned(),
        };
        assert_eq!(result, expected);

        assert!(
            find_early_vm(&xml_path, &cid_range, "vm_demo_duplicated_name").is_err(),
            "should fail"
        );
        assert!(
            find_early_vm(&xml_path, &cid_range, "vm_demo_invalid_cid_1").is_err(),
            "should fail"
        );
        assert!(
            find_early_vm(&xml_path, &cid_range, "vm_demo_invalid_cid_2").is_err(),
            "should fail"
        );

        Ok(())
    }
}
