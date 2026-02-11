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

use crate::aidl;
use crate::atom::{write_vm_booted_stats, write_vm_creation_stats};
use crate::crosvm::{
    CrosvmCommand, CrosvmConfig, PayloadState, RunContext, SharedMemoryId, SharedPathConfig,
    VmInstance, VmState,
};
use crate::debug_config::{DebugConfig, DebugPolicy};
use crate::dt_overlay::{create_device_tree_overlay, VM_DT_OVERLAY_MAX_SIZE, VM_DT_OVERLAY_PATH};
use crate::host_services;
use crate::payload::{
    add_microdroid_payload_images, add_microdroid_system_images, add_microdroid_vendor_image,
    ApexInfoList,
};
use crate::secretkeeper::{self, SecretkeeperProxy, IDENTIFIER as SECRETKEEPER_IDENTIFIER};
use crate::selinux::{
    check_host_service_permission, check_tee_service_permission, getfilecon, getprevcon, SeContext,
};
use crate::{get_calling_pid, get_calling_uid};
use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::ICEStoreKEK::ICEStoreKEK;
use anyhow::{anyhow, bail, ensure, Context, Result};
use apkverify::{HashAlgorithm, V4Signature};
use avflog::LogResult;
use binder::{
    self, wait_for_interface, Accessor, BinderFeatures, ConnectionInfo, DeathRecipient,
    ExceptionCode, IBinder, Interface, IntoBinderResult, ParcelFileDescriptor, SpIBinder, Status,
    StatusCode, Strong,
};
use glob::glob;
use libc::{sa_family_t, sockaddr_vm, AF_VSOCK};
use log::{debug, error, info, warn};
use microdroid_payload_config::{ApexConfig, ApkConfig, Task, TaskType, VmPayloadConfig};
use rpc_servicemanager_aidl::aidl::android::os::IRpcProvider::{
    BnRpcProvider, IRpcProvider, ServiceConnectionInfo::ServiceConnectionInfo, Vsock::Vsock,
};
use rpcbinder::RpcServer;
use rustutils::android::system_properties;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::ffi::{CStr, CString};
use std::fs;
use std::fs::{create_dir_all, read_dir, remove_dir_all, remove_file, File};
use std::io::{Error, ErrorKind, Seek, SeekFrom, Write};
use std::iter;
use std::num::NonZeroU16;
use std::ops::{Deref, Range};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;
use vbmeta::VbMetaImage;
use vmconfig::VmConfig;
use vsock::VsockStream;
use zip::ZipArchive;
use android_hardware_virtualization_capabilities_capabilities_service::aidl::android::hardware::virtualization::capabilities::IVmCapabilitiesService::IVmCapabilitiesService;

/// The unique ID of a VM used (together with a port number) for vsock communication.
pub type Cid = u32;

pub const BINDER_SERVICE_IDENTIFIER: &str = "android.system.virtualizationservice";

/// Magic string for the instance image
const ANDROID_VM_INSTANCE_MAGIC: &str = "Android-VM-instance";

/// Version of the instance image format
const ANDROID_VM_INSTANCE_VERSION: u16 = 1;

const MICRODROID_OS_NAME: &str = "microdroid";

const VM_CAPABILITIES_HAL_IDENTIFIER: &str =
    "android.hardware.virtualization.capabilities.IVmCapabilitiesService/default";

const UNFORMATTED_STORAGE_MAGIC: &str = "UNFORMATTED-STORAGE";

/// crosvm requires all partitions to be a multiple of 4KiB.
const PARTITION_GRANULARITY_BYTES: u64 = 4096;

const VM_REFERENCE_DT_ON_HOST_PATH: &str = "/proc/device-tree/avf/reference";

const VM_DEBUG_POLICY_OVERLAY_FILE_NAME: &str = "debug_policy.dtbo";

const ROOT_OF_TRUST_PROPS: [&str; 4] = [
    "ro.boot.vbmeta.device_state",
    "ro.boot.vbmeta.digest",
    "ro.boot.vbmeta.public_key_digest",
    "ro.boot.verifiedbootstate",
];

const ROOT_OF_TRUST_PROP_PREFIX: &str = "ro.boot.";
const ROOT_OF_TRUST_PROP_HOST_PREFIX: &str = "host.";

const SECURITY_VM_INSTANCE_ID: &[u8; 64] =
    include_bytes!(concat!(env!("OUT_DIR"), "/security_vm_instance_id"));

static GLOBAL_SERVICE: Mutex<Option<Strong<dyn aidl::IVirtualizationServiceInternal>>> =
    Mutex::new(None);

pub fn global_service() -> Option<Strong<dyn aidl::IVirtualizationServiceInternal>> {
    use binder::IBinder; // for ping_binder

    if cfg!(early) {
        return None;
    }

    let mut service = GLOBAL_SERVICE.lock().unwrap();
    if service.is_none() || service.as_ref().unwrap().as_binder().ping_binder().is_err() {
        *service = Some(
            wait_for_interface(BINDER_SERVICE_IDENTIFIER)
                .expect("Could not connect to {BINDER_SERVICE_IDENTIFIER}"),
        );
    }
    Some(service.as_ref().unwrap().clone())
}

static SUPPORTED_OS_NAMES: LazyLock<HashSet<String>> =
    LazyLock::new(|| get_supported_os_names().expect("Failed to get list of supported os names"));

static CALLING_EXE_PATH: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let calling_exe_link = format!("/proc/{}/exe", get_calling_pid());
    match fs::read_link(&calling_exe_link) {
        Ok(calling_exe_path) => Some(calling_exe_path),
        Err(e) => {
            // virtmgr forked from apps fails to read /proc probably due to hidepid=2. As we
            // discourage vendor apps, regarding such cases as system is safer.
            // TODO(b/383969737): determine if this is okay. Or find a way how to track origins of
            // apps.
            warn!("can't read_link '{calling_exe_link}': {e:?}; regarding as system");
            None
        }
    }
});

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
    // if the Merkle tree already describes the contents of the APK.
    if output.metadata()?.len() > 0 {
        if let Ok(out_sig) = V4Signature::from_idsig(&mut output) {
            if out_sig.hashing_info == sig.hashing_info {
                debug!("idsig {output:?} is up-to-date with apk {input:?}.");
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
    /// The VMs which have been created. When VMs are created a weak reference is added to this
    /// list while a strong reference is returned to the caller over Binder. Once all copies of
    /// the Binder client are dropped the weak reference here will become invalid, and will be
    /// removed from the list opportunistically the next time `add_vm` is called.
    pub vms: Mutex<Vec<Weak<VmInstance>>>,
}

impl Interface for VirtualizationService {
    fn dump(&self, writer: &mut dyn Write, _args: &[&CStr]) -> Result<(), StatusCode> {
        check_permission("android.permission.DUMP").or(Err(StatusCode::PERMISSION_DENIED))?;
        let vms = self.get_vms();
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

fn is_shutting_down() -> binder::Result<bool> {
    let shutdown_sysprop =
        system_properties::read("sys.shutdown.requested").or_service_specific_exception(-1)?;
    if shutdown_sysprop.is_none() {
        return Ok(false);
    }
    Ok(!shutdown_sysprop.unwrap().is_empty())
}

impl aidl::IVirtualizationService for VirtualizationService {
    /// Creates (but does not start) a new VM with the given configuration, assigning it the next
    /// available CID.
    ///
    /// Returns a binder `IVirtualMachine` object referring to it, as a handle for the client.
    fn createVm(
        &self,
        config: &aidl::VirtualMachineConfig,
        console_out_fd: Option<&ParcelFileDescriptor>,
        console_in_fd: Option<&ParcelFileDescriptor>,
        log_fd: Option<&ParcelFileDescriptor>,
        dump_dt_fd: Option<&ParcelFileDescriptor>,
    ) -> binder::Result<Strong<dyn aidl::IVirtualMachine>> {
        let mut is_protected = false;
        let ret = self.create_vm_internal(
            config,
            console_out_fd,
            console_in_fd,
            log_fd,
            &mut is_protected,
            dump_dt_fd,
        );
        write_vm_creation_stats(config, is_protected, &ret);
        ret
    }

    /// Allocate a new instance_id to the VM
    fn allocateInstanceId(&self) -> binder::Result<[u8; 64]> {
        check_manage_access()?;
        if let Some(service) = global_service() {
            service.allocateInstanceId()
        } else {
            Err(anyhow!("early_virtmgr doesn't support allocateInstanceId"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }

    /// Initialise an empty partition image of the given size to be used as a writable partition.
    fn initializeWritablePartition(
        &self,
        image_fd: &ParcelFileDescriptor,
        size_bytes: i64,
        partition_type: aidl::PartitionType,
    ) -> binder::Result<()> {
        check_manage_access()?;
        let size_bytes = size_bytes
            .try_into()
            .with_context(|| format!("Invalid size: {size_bytes}"))
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

        use aidl::PartitionType;
        match partition_type {
            PartitionType::RAW => Ok(()),
            PartitionType::ANDROID_VM_INSTANCE => format_as_android_vm_instance(&mut image),
            PartitionType::ENCRYPTEDSTORE => format_as_encryptedstore(&mut image),
            _ => Err(Error::new(
                ErrorKind::Unsupported,
                format!("Unsupported partition type {partition_type:?}"),
            )),
        }
        .with_context(|| format!("Failed to initialize partition as {partition_type:?}"))
        .or_service_specific_exception(-1)?;

        Ok(())
    }

    fn setEncryptedStorageSize(
        &self,
        image_fd: &ParcelFileDescriptor,
        size: i64,
    ) -> binder::Result<()> {
        check_manage_access()?;

        let size = size
            .try_into()
            .with_context(|| format!("Invalid size: {size}"))
            .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT)?;
        let size = round_up(size, PARTITION_GRANULARITY_BYTES);

        let image = clone_file(image_fd)?;
        let image_size = image.metadata().unwrap().len();

        if image_size > size {
            return Err(Status::new_exception_str(
                ExceptionCode::ILLEGAL_ARGUMENT,
                Some("Can't shrink encrypted storage"),
            ));
        }
        // Reset the file length. In most filesystems, this will not allocate any physical disk
        // space, it will only change the logical size.
        image.set_len(size).context("Failed to extend file").or_service_specific_exception(-1)?;
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
    fn debugListVms(&self) -> binder::Result<Vec<aidl::VirtualMachineDebugInfo>> {
        // Delegate to the global service, including checking the debug permission.
        if let Some(service) = global_service() {
            service.debugListVms()
        } else {
            Err(anyhow!("early_virtmgr doesn't support debugListVms"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }

    /// Get a list of assignable device types.
    fn getAssignableDevices(&self) -> binder::Result<Vec<aidl::AssignableDevice>> {
        // Delegate to the global service, including checking the permission.
        if let Some(service) = global_service() {
            service.getAssignableDevices()
        } else {
            Err(anyhow!("early_virtmgr doesn't support getAssignableDevices"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }

    /// Get a list of supported OSes.
    fn getSupportedOSList(&self) -> binder::Result<Vec<String>> {
        Ok(Vec::from_iter(SUPPORTED_OS_NAMES.iter().cloned()))
    }

    /// Get printable debug policy for testing and debugging
    fn getDebugPolicy(&self) -> binder::Result<String> {
        let debug_policy = DebugPolicy::from_host();
        Ok(format!("{debug_policy:?}"))
    }

    /// Returns whether given feature is enabled
    fn isFeatureEnabled(&self, feature: &str) -> binder::Result<bool> {
        check_manage_access()?;
        Ok(avf_features::is_feature_enabled(feature))
    }

    fn enableTestAttestation(&self) -> binder::Result<()> {
        if let Some(service) = global_service() {
            service.enableTestAttestation()
        } else {
            Err(anyhow!("early_virtmgr doesn't support enableTestAttestation"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }

    fn isRemoteAttestationSupported(&self) -> binder::Result<bool> {
        check_manage_access()?;
        if let Some(service) = global_service() {
            service.isRemoteAttestationSupported()
        } else {
            Err(anyhow!("early_virtmgr doesn't support isRemoteAttestationSupported"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }

    fn isUpdatableVmSupported(&self) -> binder::Result<bool> {
        // The response is specific to Microdroid. Updatable VMs are only possible if device
        // supports Secretkeeper. Guest OS needs to use Secretkeeper based secrets. Microdroid does
        // this, however other guest OSes may do things differently.
        check_manage_access()?;
        Ok(secretkeeper::is_supported())
    }

    fn removeVmInstance(&self, instance_id: &[u8; 64]) -> binder::Result<()> {
        check_manage_access()?;
        if let Some(service) = global_service() {
            service.removeVmInstance(instance_id)
        } else {
            Err(anyhow!("early_virtmgr doesn't support removeVmInstance"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }

    fn claimVmInstance(&self, instance_id: &[u8; 64]) -> binder::Result<()> {
        check_manage_access()?;
        if let Some(service) = global_service() {
            service.claimVmInstance(instance_id)
        } else {
            Err(anyhow!("early_virtmgr doesn't support claimVmInstance"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CallingPartition {
    Odm,
    Product,
    System,
    SystemExt,
    Vendor,
    Unknown,
}

impl CallingPartition {
    fn as_str(&self) -> &'static str {
        match self {
            CallingPartition::Odm => "odm",
            CallingPartition::Product => "product",
            CallingPartition::System => "system",
            CallingPartition::SystemExt => "system_ext",
            CallingPartition::Vendor => "vendor",
            CallingPartition::Unknown => "[unknown]",
        }
    }
}

impl std::fmt::Display for CallingPartition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

fn find_partition(path: Option<&Path>) -> binder::Result<CallingPartition> {
    let Some(path) = path else {
        return Ok(CallingPartition::System);
    };
    if path.starts_with("/system/system_ext/") {
        return Ok(CallingPartition::SystemExt);
    }
    if path.starts_with("/system/product/") || path.starts_with("/mnt/product/") {
        return Ok(CallingPartition::Product);
    }
    if path.starts_with("/data/nativetest/vendor/")
        || path.starts_with("/data/nativetest64/vendor/")
        || path.starts_with("/mnt/vendor/")
    {
        return Ok(CallingPartition::Vendor);
    }

    let partition = {
        let mut components = path.components();
        let Some(std::path::Component::Normal(partition)) = components.nth(1) else {
            return Err(anyhow!("Can't find partition in '{}'", path.display()))
                .or_service_specific_exception(-1);
        };

        // If path is under /apex, find a partition of the preinstalled .apex path
        if partition == "apex" {
            let Some(std::path::Component::Normal(apex_name)) = components.next() else {
                return Err(anyhow!("Can't find apex name for '{}'", path.display()))
                    .or_service_specific_exception(-1);
            };
            let apex_info_list = ApexInfoList::load()
                .context("Failed to get apex info list")
                .or_service_specific_exception(-1)?;
            apex_info_list
                .list
                .iter()
                .find(|apex_info| apex_info.name.as_str() == apex_name)
                .map(|apex_info| apex_info.partition.to_lowercase())
                .ok_or(anyhow!("Can't find apex info for {apex_name:?}"))
                .or_service_specific_exception(-1)?
        } else {
            partition.to_string_lossy().into_owned()
        }
    };
    Ok(match partition.as_str() {
        "odm" => CallingPartition::Odm,
        "product" => CallingPartition::Product,
        "system" => CallingPartition::System,
        "system_ext" => CallingPartition::SystemExt,
        "vendor" => CallingPartition::Vendor,
        _ => {
            warn!("unknown partition for '{}'", path.display());
            CallingPartition::Unknown
        }
    })
}

impl VirtualizationService {
    pub fn init() -> VirtualizationService {
        VirtualizationService::default()
    }

    fn create_early_vm_context(
        &self,
        config: &aidl::VirtualMachineConfig,
        calling_exe_path: Option<&Path>,
        calling_partition: CallingPartition,
    ) -> binder::Result<(Cid, PathBuf)> {
        let name = match config {
            aidl::VirtualMachineConfig::RawConfig(config) => &config.name,
            aidl::VirtualMachineConfig::AppConfig(config) => &config.name,
        };
        let early_vm = find_early_vm_for_partition(calling_partition, name)
            .or_service_specific_exception(-1)?;
        let calling_exe_path = match calling_exe_path {
            Some(path) => path,
            None => {
                return Err(anyhow!("Can't verify the path of PID {}", get_calling_pid()))
                    .or_service_specific_exception(-1)
            }
        };
        early_vm.check_exe_paths_match(calling_exe_path)?;

        let cid = early_vm.cid as Cid;
        let temp_dir = PathBuf::from(format!("/mnt/vm/early/{cid}"));
        let _ = remove_dir_all(&temp_dir);
        create_dir_all(&temp_dir)
            .context(format!("can't create '{}'", temp_dir.display()))
            .or_service_specific_exception(-1)?;

        Ok((cid, temp_dir))
    }

    fn create_vm_context(&self) -> binder::Result<(Cid, PathBuf)> {
        let vm_context = global_service().unwrap().allocateVmContext()?;
        let cid = vm_context.cid as Cid;
        let temp_dir: PathBuf = vm_context.tempDir.clone().into();
        Ok((cid, temp_dir))
    }

    fn create_vm_internal(
        &self,
        config: &aidl::VirtualMachineConfig,
        console_out_fd: Option<&ParcelFileDescriptor>,
        console_in_fd: Option<&ParcelFileDescriptor>,
        log_fd: Option<&ParcelFileDescriptor>,
        is_protected: &mut bool,
        dump_dt_fd: Option<&ParcelFileDescriptor>,
    ) -> binder::Result<Strong<dyn aidl::IVirtualMachine>> {
        let requester_uid = get_calling_uid();
        let requester_debug_pid = get_calling_pid();

        let calling_partition = find_partition(CALLING_EXE_PATH.as_deref())?;

        let instance_id = extract_instance_id(config);

        let encrypted_store_kek = extract_encrypted_store_kek(config);
        // Determine whether a VM config requires VirtualMachineService running on the host. Only
        // Microdroid VM (i.e. AppConfig) requires it. However, a few Microdroid tests use
        // RawConfig for Microdroid VM. And also Debian VM supports GuestAgent as well.
        // To handle the exceptional case, we use name as a second criteria;
        // if the name is "microdroid" or "debian", we run VirtualMachineService
        let requires_vm_service = match config {
            aidl::VirtualMachineConfig::AppConfig(_) => true,
            aidl::VirtualMachineConfig::RawConfig(config) => {
                config.name == "microdroid" || config.name == "debian"
            }
        };
        // Require vendor instance IDs to start with a specific prefix so that they don't conflict
        // with system instance IDs.
        //
        // We should also make sure that non-vendor VMs do not use the vendor prefix, but there are
        // already system VMs in the wild that may have randomly generated IDs with the prefix, so,
        // for now, we only check in one direction.
        const INSTANCE_ID_VENDOR_PREFIX: &[u8] = &[0xFF, 0xFF, 0xFF, 0xFF];
        if matches!(calling_partition, CallingPartition::Vendor | CallingPartition::Odm)
            && !instance_id.starts_with(INSTANCE_ID_VENDOR_PREFIX)
        {
            return Err(anyhow!(
                "vendor initiated VMs must have instance IDs starting with 0xFFFFFFFF, got {}",
                hex::encode(instance_id)
            ))
            .or_service_specific_exception(-1);
        }

        check_config_features(config)?;

        if cfg!(early) {
            check_config_allowed_for_early_vms(config)?;
        }

        let caller_secontext = getprevcon().or_service_specific_exception(-1)?;
        info!("callers secontext: {caller_secontext}");

        // Allocating VM context checks the MANAGE_VIRTUAL_MACHINE permission.
        let (cid, temporary_directory) = if cfg!(early) {
            self.create_early_vm_context(config, CALLING_EXE_PATH.as_deref(), calling_partition)?
        } else {
            self.create_vm_context()?
        };

        if is_custom_config(config) {
            check_use_custom_virtual_machine()?;
        }

        let mut device_tree_overlays = vec![];
        if let Some(dt_overlay) =
            maybe_create_reference_dt_overlay(config, &instance_id, &temporary_directory)?
        {
            device_tree_overlays.push(dt_overlay);
        }

        let debug_config = DebugConfig::new(config);
        if let Some(dt_overlay) =
            maybe_create_debug_policy_overlay(&debug_config, &temporary_directory)?
        {
            device_tree_overlays.push(dt_overlay);
        }

        let (is_app_config, config) = match config {
            aidl::VirtualMachineConfig::RawConfig(config) => {
                (false, BorrowedOrOwned::Borrowed(config))
            }
            aidl::VirtualMachineConfig::AppConfig(config) => {
                let config = load_app_config(config, &debug_config, &temporary_directory)
                    .or_service_specific_exception_with(-1, |e| {
                        *is_protected = config.protectedVm;
                        let message = format!("Failed to load app config: {e:?}");
                        error!("{message}");
                        message
                    })?;
                (true, BorrowedOrOwned::Owned(config))
            }
        };
        let config = config.as_ref();
        *is_protected = config.protectedVm;

        if !config.teeServices.is_empty() {
            if !config.protectedVm {
                return Err(anyhow!("only protected VMs can request tee services"))
                    .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
            }
            check_tee_service_permission(&caller_secontext, &config.teeServices)
                .with_log()
                .or_binder_exception(ExceptionCode::SECURITY)?;
        }

        let mut vendor_tee_services = Vec::new();
        for tee_service in config.teeServices.clone() {
            if tee_service.starts_with("vendor.") {
                vendor_tee_services.push(tee_service);
            }
        }

        if !vendor_tee_services.is_empty() && !is_vm_capabilities_hal_supported() {
            return Err(anyhow!(
                "requesting access to tee services requires {VM_CAPABILITIES_HAL_IDENTIFIER}"
            ))
            .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }

        // Verify the VM owner has permissions to get all of these host services
        // now to get early feedback. Each individual service is checked again
        // when the client in the VM is requesting the specific services.
        if !config.hostServices.is_empty() {
            check_host_service_permission(
                &caller_secontext,
                &config.hostServices,
                requester_debug_pid,
                requester_uid,
            )
            .with_log()
            .or_binder_exception(ExceptionCode::SECURITY)?;
        }
        let kernel = maybe_clone_file(config.kernel.as_ref())?;
        let initrd = maybe_clone_file(config.initrd.as_ref())?;

        if config.protectedVm {
            // Fail fast with a meaningful error message in case device doesn't support pVMs.
            check_protected_vm_is_supported()?;

            // In a protected VM, we require custom kernels to come from a trusted source
            // (b/237054515).
            check_label_for_kernel_files(kernel.as_ref(), initrd.as_ref(), calling_partition)
                .or_service_specific_exception(-1)?;

            // Check if partition images are labeled incorrectly. This is to prevent random images
            // which are not protected by the Android Verified Boot (e.g. bits downloaded by apps)
            // from being loaded in a pVM. This applies to everything but the instance image in the
            // raw config, and everything but the non-executable, generated partitions in the app
            // config.
            config
                .disks
                .iter()
                .flat_map(|disk| disk.image.as_ref())
                .try_for_each(|image| check_label_for_file(image, "disk image", calling_partition))
                .or_service_specific_exception(-1)?;
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
                .try_for_each(|partition| check_label_for_partition(partition, calling_partition))
                .or_service_specific_exception(-1)?;
        }

        // Check if files for payloads and bases are on the same side of the Treble boundary as the
        // calling process, as they may have unstable interfaces.
        check_partitions_for_files(config, calling_partition).or_service_specific_exception(-1)?;

        let shared_paths = assemble_shared_paths(&config.sharedPaths, &temporary_directory)?;

        let gdb_port = NonZeroU16::new(config.gdbPort as u16);
        let detect_hangup = is_app_config && gdb_port.is_none();

        // TODO(ioffe): query the sysprop here to enable VM boot tracing.
        // TODO(ioffe): also provide a way to configure what trace events should be enabled during
        //  kernel init.
        let context = RunContext {
            config,
            debug_config: &debug_config,
            cid,
            temp_dir: &temporary_directory,
            console_out: console_out_fd,
            console_in: console_in_fd,
            log_out: log_fd,
            devicetree_dump_out: dump_dt_fd,
        };
        let command = CrosvmCommand::build_from(&context).or_service_specific_exception(-1)?;

        // Actually start the VM.
        let crosvm_config = CrosvmConfig {
            cid,
            name: config.name.clone(),
            shared_paths,
            protected: *is_protected,
            detect_hangup,
            device_tree_overlays,
            start_suspended: !vendor_tee_services.is_empty(),
            command,
        };

        let instance = Arc::new(
            VmInstance::new(
                crosvm_config,
                temporary_directory,
                requester_uid,
                requester_debug_pid,
                requires_vm_service,
                vendor_tee_services,
                config.hostServices.clone(),
                encrypted_store_kek,
            )
            .with_context(|| format!("Failed to create VM with config {config:?}"))
            .with_log()
            .or_service_specific_exception(-1)?,
        );
        {
            let mut vms = self.vms.lock().unwrap();
            // Garbage collect any entries from the stored list which no longer exist.
            vms.retain(|vm| vm.strong_count() > 0);
            // Actually add the new VM.
            vms.push(Arc::downgrade(&instance));
        }

        let vm = VirtualMachine::create(instance);

        register_to_global_service(&vm)
            .context(format!("Failed to register VM ({cid}) to the global service"))
            .or_service_specific_exception(-1)?;

        Ok(vm)
    }

    pub fn get_vms(&self) -> Vec<Arc<VmInstance>> {
        self.vms.lock().unwrap().iter().filter_map(Weak::upgrade).collect()
    }
}

/// Register the VM to the global service so that the list of all VMs in the system can be
/// retrieved via the global service. Also set up a death recipient so that the VMs are "re"
/// registered to the global service if the service goes down and then is brought up again.
fn register_to_global_service(vm: &Strong<dyn aidl::IVirtualMachine>) -> binder::Result<()> {
    if cfg!(early) {
        return Ok(());
    }

    let instance = &to_local_object(vm).expect("not a local object").instance;
    let cid = instance.cid;
    let gs = global_service().unwrap();

    {
        // If we are re-registering (from DeathRecipient below), make sure the VM is still alive,
        // otherwise we could race with a unregisterVirtualMachine call (in monitor_vm_exit) and
        // end up with a zombie reference in virtualizationservice.
        let vm_state = instance.vm_state.lock().unwrap();
        if matches!(&*vm_state, VmState::Dead | VmState::Failed) {
            return Ok(());
        }

        gs.registerVirtualMachine(cid.try_into().unwrap(), vm)?;

        // Hold the lock until after `registerVirtualMachine` to ensure that an "alive -> dead"
        // transition (and `unregisterVirtualMachine` call) can't occur until afterwards.
    }

    let gs_clone = gs.clone();
    let weak_vm = Strong::downgrade(vm);
    let mut dr = DeathRecipient::new(move || {
        // Hold a strong ref to the global service in the callback, otherwise the first
        // `DeathRecipient` that gets to run will cause the only strong ref (in `GLOBAL_SERVICE`)
        // to be dropped and so other `DeathRecipient`s will be skipped when `link_to_death`'s weak
        // ref fails to promote.
        let _ = &gs_clone;
        // No need to re-register the VM if it's already dead
        if let Ok(vm) = weak_vm.upgrade() {
            if let Err(e) = register_to_global_service(&vm) {
                error!("Failed to re-register VM (cid) to the global service: {e:?}");
            }
        }
    });
    gs.as_binder().link_to_death(&mut dr)?;

    // Hold DeathRecipient in VmInstance. We need this because if DeathRecipient is dropped, it is
    // automatically unlinked.
    *instance.global_service_death_recipient.lock().unwrap() = Some(dr);
    Ok(())
}

/// Returns whether a VM config represents a "custom" virtual machine, which requires the
/// USE_CUSTOM_VIRTUAL_MACHINE.
fn is_custom_config(config: &aidl::VirtualMachineConfig) -> bool {
    match config {
        // Any raw (non-Microdroid) VM is considered custom.
        aidl::VirtualMachineConfig::RawConfig(_) => true,
        aidl::VirtualMachineConfig::AppConfig(config) => {
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
                    aidl::Payload::ConfigPath(_) => true,
                    aidl::Payload::PayloadConfig(payload_config) => {
                        !payload_config.extraApks.is_empty()
                    }
                }) || config.osName != MICRODROID_OS_NAME
            }
        }
    }
}

fn extract_vendor_hashtree_digest(config: &aidl::VirtualMachineConfig) -> Result<Option<Vec<u8>>> {
    let aidl::VirtualMachineConfig::AppConfig(config) = config else {
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

fn maybe_create_reference_dt_overlay(
    config: &aidl::VirtualMachineConfig,
    instance_id: &[u8; 64],
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

    let mut trusted_props = if let Some(ref vendor_hashtree_digest) = vendor_hashtree_digest {
        info!(
            "Passing vendor hashtree digest to pvmfw. This will be rejected if it doesn't \
                match the trusted digest in the pvmfw config, causing the VM to fail to start."
        );
        vec![(c"vendor_hashtree_descriptor_root_digest", vendor_hashtree_digest.as_slice())]
    } else {
        vec![]
    };

    let key_material;
    let mut untrusted_props = Vec::with_capacity(2);
    if cfg!(llpvm_changes) {
        untrusted_props.push((c"instance-id", &instance_id[..]));
        let want_updatable = extract_want_updatable(config);
        if want_updatable && secretkeeper::is_supported() {
            // Let guest know that it can defer rollback protection to Secretkeeper by setting
            // an empty property in untrusted node in DT. This enables Updatable VMs.
            untrusted_props.push((c"defer-rollback-protection", &[]));
            let sk: Strong<dyn aidl::ISecretkeeper> =
                binder::wait_for_interface(SECRETKEEPER_IDENTIFIER)?;
            if sk.getInterfaceVersion()? >= 2 {
                let aidl::PublicKey { keyMaterial } = sk.getSecretkeeperIdentity()?;
                key_material = keyMaterial;
                trusted_props.push((c"secretkeeper_public_key", key_material.as_slice()));
            }
        }
    }

    let mut android_firmware_props_owned = Vec::new();

    // Populate the root of trust properties for Keymint.
    if cfg!(forward_rot) && extract_instance_id(config) == *SECURITY_VM_INSTANCE_ID {
        for prop_name in ROOT_OF_TRUST_PROPS {
            if let Some(prop_value) = system_properties::read(prop_name)
                .map_err(|e| anyhow!("Failed to read system property, {e:?}"))
                .or_service_specific_exception(-1)?
            {
                let prop_name = prop_name.strip_prefix(ROOT_OF_TRUST_PROP_PREFIX).unwrap();
                let prop_name = format!("{ROOT_OF_TRUST_PROP_HOST_PREFIX}{prop_name}");
                android_firmware_props_owned
                    .push((CString::new(prop_name).unwrap(), CString::new(prop_value).unwrap()));
            }
        }
    }

    let device_tree_overlay = if host_ref_dt.is_some()
        || !untrusted_props.is_empty()
        || !trusted_props.is_empty()
        || !android_firmware_props_owned.is_empty()
    {
        let dt_output = temporary_directory.join(VM_DT_OVERLAY_PATH);
        let mut data = [0_u8; VM_DT_OVERLAY_MAX_SIZE];
        let android_firmware_props = android_firmware_props_owned
            .iter()
            .map(|(n, v)| (n.as_c_str(), v.as_bytes_with_nul()))
            .collect::<Vec<_>>();
        let fdt = create_device_tree_overlay(
            &mut data,
            host_ref_dt,
            &untrusted_props,
            &trusted_props,
            &android_firmware_props,
        )
        .map_err(|e| anyhow!("Failed to create DT overlay, {e:?}"))
        .or_service_specific_exception(-1)?;
        fs::write(&dt_output, fdt.as_slice()).or_service_specific_exception(-1)?;
        Some(File::open(dt_output).or_service_specific_exception(-1)?)
    } else {
        None
    };
    Ok(device_tree_overlay)
}

fn maybe_create_debug_policy_overlay(
    config: &DebugConfig,
    temporary_directory: &Path,
) -> binder::Result<Option<File>> {
    if let Some(fdt_overlay) = config.get_debug_policy_overlay() {
        let dt_output = temporary_directory.join(VM_DEBUG_POLICY_OVERLAY_FILE_NAME);
        fs::write(&dt_output, fdt_overlay.as_slice())
            .map_err(|e| anyhow!("Failed to create DT overlay, {dt_output:?}, e={e:?}"))
            .or_service_specific_exception(-1)?;
        Ok(Some(
            File::open(&dt_output)
                .map_err(|e| anyhow!("Failed to open DT overlay, {dt_output:?}, e={e:?}"))
                .or_service_specific_exception(-1)?,
        ))
    } else {
        Ok(None)
    }
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

fn assemble_shared_paths(
    shared_paths: &[aidl::SharedPath],
    temporary_directory: &Path,
) -> Result<Vec<SharedPathConfig>, Status> {
    if shared_paths.is_empty() {
        return Ok(Vec::new()); // Return an empty vector if shared_paths is empty
    }

    shared_paths
        .iter()
        .map(|path| {
            Ok(SharedPathConfig {
                path: path.sharedPath.clone(),
                host_uid: path.hostUid,
                host_gid: path.hostGid,
                guest_uid: path.guestUid,
                guest_gid: path.guestGid,
                mask: path.mask,
                tag: path.tag.clone(),
                socket_path: temporary_directory
                    .join(&path.socketPath)
                    .to_string_lossy()
                    .to_string(),
            })
        })
        .collect()
}

fn append_kernel_param(param: &str, vm_config: &mut aidl::VirtualMachineRawConfig) {
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

fn load_app_config(
    config: &aidl::VirtualMachineAppConfig,
    debug_config: &DebugConfig,
    temporary_directory: &Path,
) -> Result<aidl::VirtualMachineRawConfig> {
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
        aidl::Payload::ConfigPath(config_path) => {
            vm_payload_config =
                load_vm_payload_config_from_file(&apk_file, config_path.as_str())
                    .with_context(|| format!("Couldn't read config from {config_path}"))?;
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
        aidl::Payload::PayloadConfig(payload_config) => {
            vm_payload_config = create_vm_payload_config(payload_config)?;
            extra_apk_files =
                payload_config.extraApks.iter().map(clone_file).collect::<binder::Result<_>>()?;
        }
    };

    if !cfg!(advance_multitenancy) {
        check_no_additional_tenants(&vm_payload_config)?;
    }

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
    let vm_config_path = PathBuf::from(format!("/apex/com.android.virt/etc/{os_name}.json"));
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

        vm_config.devices = aidl::AssignedDevices::Devices(custom_config.devices.clone());
        vm_config.networkSupported = custom_config.networkSupported;

        for param in custom_config.extraKernelCmdlineParams.iter() {
            append_kernel_param(param, &mut vm_config);
        }

        vm_config.teeServices.clone_from(&custom_config.teeServices);

        vm_config.gdbPort = custom_config.gdbPort;
    }

    if config.memoryMib > 0 {
        vm_config.memoryMib = config.memoryMib;
    }

    if os_name.ends_with("_16k") && cfg!(target_arch = "x86_64") {
        append_kernel_param("page_shift=14", &mut vm_config);
    }

    vm_config.name.clone_from(&config.name);
    vm_config.protectedVm = config.protectedVm;
    vm_config.cpuOptions = config.cpuOptions.clone();
    vm_config.hugePages = config.hugePages || vm_payload_config.hugepages;
    vm_config.boostUclamp = config.boostUclamp;
    vm_config.hostServices = config.hostServices.clone();
    vm_config.osName = config.osName.clone();
    vm_config.instanceId = config.instanceId;

    vm_config.allowVgicItsInPvm = avf_aconfig::microdroid_pvm_gic_its();

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

fn check_partition_for_file(
    fd: &ParcelFileDescriptor,
    calling_partition: CallingPartition,
) -> Result<()> {
    let path = format!("/proc/self/fd/{}", fd.as_raw_fd());
    let link = fs::read_link(&path).with_context(|| format!("can't read_link {path}"))?;

    // microdroid vendor image is OK
    if cfg!(vendor_modules) && link == Path::new("/vendor/etc/avf/microdroid/microdroid_vendor.img")
    {
        return Ok(());
    }

    let fd_partition = find_partition(Some(&link))
        .with_context(|| format!("can't find_partition {}", link.display()))?;
    let is_fd_vendor =
        fd_partition == CallingPartition::Vendor || fd_partition == CallingPartition::Odm;
    let is_caller_vendor =
        calling_partition == CallingPartition::Vendor || calling_partition == CallingPartition::Odm;

    if is_fd_vendor != is_caller_vendor {
        bail!("{} can't be used for VM client in {calling_partition}", link.display());
    }

    Ok(())
}

fn check_partitions_for_files(
    config: &aidl::VirtualMachineRawConfig,
    calling_partition: CallingPartition,
) -> Result<()> {
    config
        .disks
        .iter()
        .flat_map(|disk| disk.partitions.iter())
        .filter_map(|partition| partition.image.as_ref())
        .try_for_each(|fd| check_partition_for_file(fd, calling_partition))?;

    config
        .disks
        .iter()
        .filter_map(|disk| disk.image.as_ref())
        .try_for_each(|fd| check_partition_for_file(fd, calling_partition))?;

    config.kernel.as_ref().map_or(Ok(()), |fd| check_partition_for_file(fd, calling_partition))?;
    config.initrd.as_ref().map_or(Ok(()), |fd| check_partition_for_file(fd, calling_partition))?;
    config
        .bootloader
        .as_ref()
        .map_or(Ok(()), |fd| check_partition_for_file(fd, calling_partition))?;

    Ok(())
}

fn load_vm_payload_config_from_file(apk_file: &File, config_path: &str) -> Result<VmPayloadConfig> {
    let mut apk_zip = ZipArchive::new(apk_file)?;
    let config_file = apk_zip.by_name(config_path)?;
    Ok(serde_json::from_reader(config_file)?)
}

fn create_vm_payload_config(
    payload_config: &aidl::VirtualMachinePayloadConfig,
) -> Result<VmPayloadConfig> {
    // There isn't an actual config file. Construct a synthetic VmPayloadConfig from the explicit
    // parameters we've been given. Microdroid will do something equivalent inside the VM using the
    // payload config that we send it via the metadata file.

    let payload_binary_name = &payload_config.payloadBinaryName;
    if payload_binary_name.contains('/') {
        bail!("Payload binary name must not specify a path: {payload_binary_name}");
    }

    let task = Task {
        type_: TaskType::MicrodroidLauncher,
        command: payload_binary_name.clone(),
        command_args: None,
        selinux_type: None,
    };

    // The VM only cares about how many there are, these names are actually ignored.
    let extra_apk_count = payload_config.extraApks.len();
    let extra_apks =
        (0..extra_apk_count).map(|i| ApkConfig { path: format!("extra-apk-{i}") }).collect();

    if check_use_relaxed_microdroid_rollback_protection().is_ok() {
        // The only payload delivered via Mainline module in this release requires
        // com.android.i18n apex. However, we are past all the reasonable deadlines to add new
        // APIs, so we use the fact that the payload is granted
        // USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION permission as a signal that we should include
        // com.android.i18n APEX.
        // TODO: remove this after we provide a stable @SystemApi to load additional APEXes to
        // Microdroid pVMs. Note - changing this breaks existing DICE chain.
        // b/406856088 - added appsearch here as well, which is needed for .so shareing.
        let apexes = vec![
            ApexConfig { name: String::from("com.android.i18n") },
            ApexConfig { name: String::from("com.android.appsearch") },
        ];
        Ok(VmPayloadConfig { task: Some(task), apexes, extra_apks, ..Default::default() })
    } else {
        Ok(VmPayloadConfig { task: Some(task), extra_apks, ..Default::default() })
    }
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
    let perm_svc: Strong<dyn aidl::IPermissionController> =
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

/// Check whether the caller of the current binder method is allowed to use relaxed microdroid
/// rollback protection schema.
pub fn check_use_relaxed_microdroid_rollback_protection() -> binder::Result<()> {
    check_permission("android.permission.USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION")
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
        || label.starts_with("tenant-idsig-")
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
fn check_label_is_allowed(context: &SeContext, calling_partition: CallingPartition) -> Result<()> {
    match context.selinux_type()? {
        | "apk_data_file" // APKs of an installed app
        | "shell_data_file" // test files created via adb shell
        | "staging_data_file" // updated/staged APEX images
        | "apex_dm_device" // updated APEX images when mount_before_data is enabled
        | "system_file" // immutable dm-verity protected partition
        | "virtualizationservice_data_file" // files created by VS / VirtMgr
        | "vendor_microdroid_file" // immutable dm-verity protected partition (/vendor/etc/avf/microdroid/.*)
         => Ok(()),
        // It is difficult to require specific label types for vendor initiated VM's files.
        _ if calling_partition == CallingPartition::Vendor => Ok(()),
        _ => bail!("Label {} is not allowed", context),
    }
}

fn check_label_for_partition(
    partition: &aidl::Partition,
    calling_partition: CallingPartition,
) -> Result<()> {
    let file = partition.image.as_ref().unwrap().as_ref();
    check_label_is_allowed(&getfilecon(file)?, calling_partition)
        .with_context(|| format!("Partition {} invalid", &partition.label))
}

fn check_label_for_kernel_files(
    kernel: Option<&File>,
    initrd: Option<&File>,
    calling_partition: CallingPartition,
) -> Result<()> {
    if let Some(f) = kernel {
        check_label_for_file(f, "kernel", calling_partition)?;
    }
    if let Some(f) = initrd {
        check_label_for_file(f, "initrd", calling_partition)?;
    }
    Ok(())
}
fn check_label_for_file(
    file: &impl AsRawFd,
    name: &str,
    calling_partition: CallingPartition,
) -> Result<()> {
    check_label_is_allowed(&getfilecon(file)?, calling_partition)
        .with_context(|| format!("{name} file invalid"))
}

/// Implementation of the AIDL `IVirtualMachine` interface. Used as a handle to a VM.
struct VirtualMachine {
    instance: Arc<VmInstance>,
}

impl VirtualMachine {
    fn create(instance: Arc<VmInstance>) -> Strong<dyn aidl::IVirtualMachine> {
        aidl::BnVirtualMachine::new_binder(VirtualMachine { instance }, BinderFeatures::default())
    }

    fn handle_vendor_tee_services(&self) -> binder::Result<()> {
        self.handle_vendor_tee_services_internal().with_log().or_service_specific_exception(-1)
    }

    fn handle_vendor_tee_services_internal(&self) -> Result<()> {
        let vm_fd = self
            .instance
            .get_vm_fd()
            .with_context(|| format!("Error getting fd of VM with CID {}", self.instance.cid))?;

        let vm_caps_hal: Strong<dyn IVmCapabilitiesService> =
            binder::wait_for_interface(VM_CAPABILITIES_HAL_IDENTIFIER)?;

        let vm_pfd = ParcelFileDescriptor::new(vm_fd);
        vm_caps_hal
            .grantAccessToVendorTeeServices(&vm_pfd, &self.instance.vendor_tee_services)
            .context("grantAccessToVendorTeeServices failed")?;

        self.instance
            .resume_full()
            .with_context(|| format!("Error resuming VM with CID {}", self.instance.cid))
    }
}

/// Converts a strong binder reference to a local object if the reference is local. This function
/// actually doesn't return the local object, but a holder object (VirtualMachineBinder) which is
/// automatically dereferenced to the local object
fn to_local_object(
    binder: &Strong<dyn aidl::IVirtualMachine>,
) -> Result<impl Deref<Target = VirtualMachine>> {
    let binder = binder.as_binder().try_into().context("Not a local reference")?;
    Ok(VirtualMachineBinder(binder))
}

/// Validates that a vsock port is either unprivileged (>= 1024) or within the allowed exception
/// range for Trusty VMs.
fn validate_vsock_port(port: u32) -> binder::Result<()> {
    /// Vsock privileged ports are below this number.
    const VSOCK_PRIV_PORT_MAX: u32 = 1024;
    /// Min vsock port number used by Trusty VMs. See b/427392420 for more context.
    const TRUSTY_VSOCK_PORT_MIN: u32 = 2;
    /// Max vsock port number used by Trusty VMs.
    const TRUSTY_VSOCK_PORT_MAX: u32 = 20;

    let is_unprivileged = port >= VSOCK_PRIV_PORT_MAX;
    let is_allowed_trusty_port = (TRUSTY_VSOCK_PORT_MIN..=TRUSTY_VSOCK_PORT_MAX).contains(&port);

    if is_unprivileged || is_allowed_trusty_port {
        Ok(())
    } else {
        Err(Status::new_service_specific_error_str(
            aidl::ERROR_UNEXPECTED,
            Some(format!("Can't connect to privileged port {port}")),
        ))
    }
}

struct VirtualMachineBinder(binder::binder_impl::Binder<aidl::BnVirtualMachine>);

impl Deref for VirtualMachineBinder {
    type Target = VirtualMachine;
    fn deref(&self) -> &Self::Target {
        self.0.downcast_binder().expect("IVirtualMachine is not implemented by VirtualMachine")
    }
}

impl Interface for VirtualMachine {
    fn dump(&self, writer: &mut dyn Write, args: &[&CStr]) -> Result<(), binder::StatusCode> {
        if !self.instance.is_vm_running() {
            writeln!(writer, "skipping dump: VM is not running")
                .or(Err(StatusCode::UNKNOWN_ERROR))?;
            return Ok(());
        }

        let args = args.iter().map(|x| x.to_string_lossy().to_string()).collect::<Vec<_>>();

        let port = {
            let Some(guest_agent) = &*self.instance.guest_agent.lock().unwrap() else {
                writeln!(writer, "skipping dump: guest agent isn't registered")
                    .or(Err(StatusCode::UNKNOWN_ERROR))?;
                return Ok(());
            };

            match guest_agent.startDumpVsockServer(&args) {
                Ok(port) => port,
                Err(e) => {
                    writeln!(writer, "skipping dump: failed to start dump vsock server: {e:?}")
                        .or(Err(StatusCode::UNKNOWN_ERROR))?;
                    return Ok(());
                }
            }
        };

        let mut stream = match VsockStream::connect_with_cid_port(self.instance.cid, port as u32) {
            Ok(stream) => stream,
            Err(e) => {
                writeln!(writer, "skipping dump: failed to connect to dump vsock server: {e:?}")
                    .or(Err(StatusCode::UNKNOWN_ERROR))?;
                return Ok(());
            }
        };

        // 5 seconds must be much longer than required.
        stream.set_read_timeout(Some(Duration::from_secs(5))).or(Err(StatusCode::UNKNOWN_ERROR))?;

        if let Err(e) = std::io::copy(&mut stream, writer) {
            writeln!(writer, "skipping dump: failed to copy dump stream: {e:?}")
                .or(Err(StatusCode::UNKNOWN_ERROR))?;
        }

        Ok(())
    }
}

impl aidl::IVirtualMachine for VirtualMachine {
    fn getCid(&self) -> binder::Result<i32> {
        // Don't check permission. The owner of the VM might have passed this binder object to
        // others.
        Ok(self.instance.cid as i32)
    }

    fn getState(&self) -> binder::Result<aidl::VirtualMachineState> {
        // Don't check permission. The owner of the VM might have passed this binder object to
        // others.
        Ok(get_state(&self.instance))
    }

    fn registerCallback(
        &self,
        callback: &Strong<dyn aidl::IVirtualMachineCallback>,
    ) -> binder::Result<()> {
        // Don't check permission. The owner of the VM might have passed this binder object to
        // others.

        // Only register callback if it may be notified.
        // This also ensures no cyclic reference between callback and VmInstance after VM is died.
        let vm_state = self.instance.vm_state.lock().unwrap();
        if matches!(*vm_state, VmState::Dead) {
            warn!("Ignoring registerCallback() after VM is died");
        } else {
            self.instance.callbacks.add(callback.clone());
        }
        drop(vm_state);

        Ok(())
    }

    fn start(&self) -> binder::Result<()> {
        if is_shutting_down()? {
            self.instance
                .callbacks
                .callback_on_died(self.instance.cid, aidl::DeathReason::HOST_SHUTDOWN);
            return Err(Status::new_service_specific_error_str(
                aidl::ERROR_UNEXPECTED,
                Some("Device is shutting down"),
            ));
        }
        if self.instance.requires_vm_service {
            let cid = self.instance.cid;
            // Start VM service listening for connections from the new CID on port=CID.
            let service = VirtualMachineService::new_binder(self.instance.clone()).as_binder();
            let port = cid;
            let (vm_server, _) = RpcServer::new_vsock(service, cid, port)
                .context(format!("Could not start RpcServer on port {port}"))
                .or_service_specific_exception(-1)?;
            vm_server.start();
            *self.instance.vm_service.lock().unwrap() = Some(vm_server);
        }
        self.instance
            .start()
            .with_context(|| format!("Error starting VM with CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)?;
        if !self.instance.vendor_tee_services.is_empty() {
            self.handle_vendor_tee_services()
        } else {
            Ok(())
        }
    }

    fn stop(&self) -> binder::Result<()> {
        self.instance
            .kill()
            .with_context(|| format!("Error stopping VM with CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)
    }

    fn isMemoryBalloonEnabled(&self) -> binder::Result<bool> {
        Ok(self.instance.balloon_enabled)
    }

    fn getActualMemoryBalloonBytes(&self) -> binder::Result<i64> {
        let balloon = self
            .instance
            .get_actual_memory_balloon_bytes()
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
            return Err(Status::new_service_specific_error_str(
                aidl::ERROR_UNEXPECTED,
                Some("Virtual Machine is not running"),
            ));
        }
        let port = port as u32;
        validate_vsock_port(port)?;
        let stream = VsockStream::connect_with_cid_port(self.instance.cid, port)
            .context("Failed to connect")
            .or_service_specific_exception(aidl::ERROR_UNEXPECTED)?;
        Ok(vsock_stream_to_pfd(stream))
    }

    fn createAccessorBinder(&self, name: &str, port: i32) -> binder::Result<SpIBinder> {
        if !matches!(&*self.instance.vm_state.lock().unwrap(), VmState::Running { .. }) {
            return Err(Status::new_service_specific_error_str(
                aidl::ERROR_UNEXPECTED,
                Some("Virtual Machine is not running"),
            ));
        }
        let port = port as u32;
        validate_vsock_port(port)?;
        let cid = self.instance.cid;
        let addr = sockaddr_vm {
            svm_family: AF_VSOCK as sa_family_t,
            svm_reserved1: 0,
            svm_port: port,
            svm_cid: cid,
            svm_zero: [0u8; 4],
        };
        let get_connection_info = move |_instance: &str| Some(ConnectionInfo::Vsock(addr));
        let accessor = Accessor::new(name, get_connection_info);
        accessor
            .as_binder()
            .context("The newly created Accessor should always have a binder")
            .or_service_specific_exception(aidl::ERROR_UNEXPECTED)
    }

    fn setHostConsoleName(&self, ptsname: &str) -> binder::Result<()> {
        if !self.instance.is_vm_running() {
            return Err(Status::new_service_specific_error_str(
                aidl::ERROR_UNEXPECTED,
                Some("Virtual Machine is not running"),
            ));
        }
        *self.instance.host_console_name.lock().unwrap() = Some(ptsname.to_string());
        Ok(())
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

    fn getDebugInfo(&self) -> binder::Result<aidl::VirtualMachineDebugInfo> {
        info!("getDebugInfo called");
        Ok(aidl::VirtualMachineDebugInfo {
            name: self.instance.name.clone(),
            cid: self.instance.cid.try_into().unwrap(),
            temporaryDirectory: self.instance.temporary_directory.to_string_lossy().to_string(),
            requesterUid: self.instance.requester_uid.try_into().unwrap(),
            requesterPid: self.instance.requester_debug_pid,
            hostConsoleName: self.instance.host_console_name.lock().unwrap().clone(),
        })
    }

    fn addMemoryToGuest(
        &self,
        fd: &ParcelFileDescriptor,
        offset: i64,
        range_start: i64,
        range_end: i64,
        cacheable: bool,
    ) -> binder::Result<i32> {
        info!("addMemoryToGuest called");
        check_memory_share_supported()?;
        let fd = clone_file(fd)?;
        let id = self
            .instance
            .add_memory(fd.into(), offset as u64, range_start as u64, range_end as u64, cacheable)
            .with_context(|| format!("add memory failed. CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)?;
        Ok(id.0)
    }

    fn removeMemoryFromGuest(&self, memory_id: i32) -> binder::Result<()> {
        info!("removeMemoryFromGuest called");
        check_memory_share_supported()?;
        let memory_id = SharedMemoryId(memory_id);
        self.instance
            .remove_memory(&memory_id)
            .with_context(|| format!("remove memory failed. CID {}", self.instance.cid))
            .with_log()
            .or_service_specific_exception(-1)
    }

    fn getGuestAgent(&self) -> binder::Result<Option<Strong<dyn aidl::IGuestAgent>>> {
        let guest_agent = self.instance.guest_agent.lock().unwrap();
        Ok(guest_agent.clone())
    }
}

impl Drop for VirtualMachine {
    fn drop(&mut self) {
        info!("Dropping {}", self.instance);
        if let Err(e) = self.instance.kill() {
            debug!("Error stopping dropped VM with CID {}: {:?}", self.instance.cid, e);
        }
    }
}

/// A set of Binders to be called back in response to various events on the VM, such as when it
/// dies.
#[derive(Debug, Default)]
pub struct VirtualMachineCallbacks(Mutex<Vec<Strong<dyn aidl::IVirtualMachineCallback>>>);

impl VirtualMachineCallbacks {
    /// Call all registered callbacks to notify that the payload has started.
    pub fn notify_payload_started(&self, cid: Cid) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onPayloadStarted(cid as i32) {
                error!("Error notifying payload start event from VM CID {cid}: {e:?}");
            }
        }
    }

    /// Call all registered callbacks to notify that the payload is ready to serve.
    pub fn notify_payload_ready(&self, cid: Cid) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onPayloadReady(cid as i32) {
                error!("Error notifying payload ready event from VM CID {cid}: {e:?}");
            }
        }
    }

    /// Call all registered callbacks to notify that the payload has finished.
    pub fn notify_payload_finished(&self, cid: Cid, exit_code: i32) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onPayloadFinished(cid as i32, exit_code) {
                error!("Error notifying payload finish event from VM CID {cid}: {e:?}");
            }
        }
    }

    /// Call all registered callbacks to say that the VM encountered an error.
    pub fn notify_error(&self, cid: Cid, error_code: aidl::ErrorCode, message: &str) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onError(cid as i32, error_code, message) {
                error!("Error notifying error event from VM CID {cid}: {e:?}");
            }
        }
    }

    /// Call all registered callbacks to say that the guest agent is registered
    pub fn notify_guest_agent_registered(
        &self,
        cid: Cid,
        guest_agent: &Strong<dyn aidl::IGuestAgent>,
    ) {
        let callbacks = &*self.0.lock().unwrap();
        for callback in callbacks {
            if let Err(e) = callback.onGuestAgentRegistered(cid as i32, guest_agent) {
                error!("Error notifying guest agent registered event from VM CID {cid}: {e:?}");
            }
        }
    }

    /// Call all registered callbacks to say that the VM has died.
    pub fn callback_on_died(&self, cid: Cid, reason: aidl::DeathReason) {
        let mut callbacks = self.0.lock().unwrap();
        for callback in &*callbacks {
            if let Err(e) = callback.onDied(cid as i32, reason) {
                error!("Error notifying exit of VM CID {cid}: {e:?}");
            }
        }

        // Nothing to notify afterward because VM cannot be restarted.
        // Explicitly clear callbacks to prevent potential cyclic references
        // between callback and VmInstance.
        (*callbacks).clear();
    }

    /// Add a new callback to the set.
    fn add(&self, callback: Strong<dyn aidl::IVirtualMachineCallback>) {
        self.0.lock().unwrap().push(callback);
    }
}

/// Gets the `VirtualMachineState` of the given `VmInstance`.
fn get_state(instance: &VmInstance) -> aidl::VirtualMachineState {
    use aidl::VirtualMachineState;
    match &*instance.vm_state.lock().unwrap() {
        VmState::NotStarted { .. } => VirtualMachineState::NOT_STARTED,
        VmState::Running { .. } => match instance.payload_state() {
            PayloadState::Starting => VirtualMachineState::STARTING,
            PayloadState::Started => VirtualMachineState::STARTED,
            PayloadState::Ready => VirtualMachineState::READY,
            PayloadState::Finished => VirtualMachineState::FINISHED,
            PayloadState::Hangup => VirtualMachineState::DEAD,
        },
        VmState::ShuttingDown { .. } => VirtualMachineState::FINISHED,
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

/// Converts an `Option<&ParcelFileDescriptor>` to an `Option<File>` by cloning the file.
fn maybe_clone_file(file: Option<&ParcelFileDescriptor>) -> binder::Result<Option<File>> {
    file.map(clone_file).transpose()
}

/// Converts a `VsockStream` to a `ParcelFileDescriptor`.
fn vsock_stream_to_pfd(stream: VsockStream) -> ParcelFileDescriptor {
    // SAFETY: ownership is transferred from stream to f
    let f = unsafe { File::from_raw_fd(stream.into_raw_fd()) };
    ParcelFileDescriptor::new(f)
}

// TODO(jiyong): remove this function
fn extract_instance_id(config: &aidl::VirtualMachineConfig) -> [u8; 64] {
    match config {
        aidl::VirtualMachineConfig::RawConfig(config) => config.instanceId,
        aidl::VirtualMachineConfig::AppConfig(config) => config.instanceId,
    }
}

fn extract_want_updatable(config: &aidl::VirtualMachineConfig) -> bool {
    match config {
        aidl::VirtualMachineConfig::RawConfig(_) => true,
        aidl::VirtualMachineConfig::AppConfig(config) => {
            let Some(custom) = &config.customConfig else { return true };
            custom.wantUpdatable
        }
    }
}

fn extract_encrypted_store_kek(
    config: &aidl::VirtualMachineConfig,
) -> Option<Strong<dyn aidl::IEncryptedStoreKEK>> {
    match config {
        aidl::VirtualMachineConfig::RawConfig(_) => None,
        aidl::VirtualMachineConfig::AppConfig(config) => config.encryptedStoreKEK.clone(),
    }
}

fn check_no_vendor_modules(config: &aidl::VirtualMachineConfig) -> binder::Result<()> {
    let aidl::VirtualMachineConfig::AppConfig(config) = config else { return Ok(()) };
    if let Some(custom_config) = &config.customConfig {
        if custom_config.vendorImage.is_some() || custom_config.customKernelImage.is_some() {
            return Err(anyhow!("vendor modules feature is disabled"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }
    }
    Ok(())
}

fn check_no_devices(config: &aidl::VirtualMachineConfig) -> binder::Result<()> {
    let aidl::VirtualMachineConfig::AppConfig(config) = config else { return Ok(()) };
    if let Some(custom_config) = &config.customConfig {
        if !custom_config.devices.is_empty() {
            return Err(anyhow!("device assignment feature is disabled"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }
    }
    Ok(())
}

fn check_no_extra_kernel_cmdline_params(config: &aidl::VirtualMachineConfig) -> binder::Result<()> {
    let aidl::VirtualMachineConfig::AppConfig(config) = config else { return Ok(()) };
    if let Some(custom_config) = &config.customConfig {
        if !custom_config.extraKernelCmdlineParams.is_empty() {
            return Err(anyhow!("debuggable_vms_improvements feature is disabled"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }
    }
    Ok(())
}

fn check_no_delay_enc_store(config: &aidl::VirtualMachineConfig) -> binder::Result<()> {
    let aidl::VirtualMachineConfig::AppConfig(config) = config else { return Ok(()) };
    if config.shouldDelayEncryptedStoreSetup {
        return Err(anyhow!("long_running_vms feature is disabled"))
            .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
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

fn check_no_additional_tenants(config: &VmPayloadConfig) -> Result<()> {
    if config.tenants.is_empty() {
        return Ok(());
    }
    Err(anyhow!("Multitenancy is not supported"))
}

fn check_no_additional_tenants_idsig(config: &aidl::VirtualMachineConfig) -> binder::Result<()> {
    let aidl::VirtualMachineConfig::AppConfig(config) = config else { return Ok(()) };
    if config.tenantIdsigs.is_empty() {
        return Ok(());
    }
    Err(anyhow!("Multitenancy is not supported"))
        .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
}

fn check_config_features(config: &aidl::VirtualMachineConfig) -> binder::Result<()> {
    if !cfg!(vendor_modules) {
        check_no_vendor_modules(config)?;
    }
    if !cfg!(device_assignment) {
        check_no_devices(config)?;
    }
    if !cfg!(debuggable_vms_improvements) {
        check_no_extra_kernel_cmdline_params(config)?;
    }
    if !cfg!(long_running_vms) {
        check_no_delay_enc_store(config)?;
    }
    if !cfg!(advance_multitenancy) {
        check_no_additional_tenants_idsig(config)?;
    }
    Ok(())
}

fn check_config_allowed_for_early_vms(config: &aidl::VirtualMachineConfig) -> binder::Result<()> {
    check_no_vendor_modules(config)?;
    check_no_devices(config)?;

    Ok(())
}

fn check_memory_share_supported() -> binder::Result<()> {
    if !hypervisor_props::is_dynamic_zero_copy_memshare_supported().unwrap_or(false) {
        Err(anyhow!("hypervisor doesn't support dynamic memory sharing with guest"))
            .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
    } else {
        Ok(())
    }
}

/// Simple utility for referencing Borrowed or Owned. Similar to std::borrow::Cow, but
/// it doesn't require that T implements Clone.
enum BorrowedOrOwned<'a, T> {
    Borrowed(&'a T),
    Owned(T),
}

impl<T> AsRef<T> for BorrowedOrOwned<'_, T> {
    fn as_ref(&self) -> &T {
        match self {
            Self::Borrowed(b) => b,
            Self::Owned(o) => o,
        }
    }
}

struct HostRpcProvider {
    vm_instance: Arc<VmInstance>,
    // Keep a map of instances to ports to know if we've already set up a proxy
    proxied_services: Mutex<HashMap<String, Vsock>>,
}

impl Interface for HostRpcProvider {}
impl IRpcProvider for HostRpcProvider {
    fn getServiceConnectionInfo(&self, name: &str) -> binder::Result<ServiceConnectionInfo> {
        let vm = &self.vm_instance;
        let cid = vm.cid;
        if !vm.host_services.iter().any(|i| i == name) {
            return Err(anyhow!(
                "This VM is not configured to access the host service \
                    {name}. The service must be added to the VM configuration in \
                    the hostServices field."
            ))
            .or_service_specific_exception(-1);
        }
        // Check with servicemanager to make sure the VM owner still has permission to get this
        // service
        let caller_secontext = getprevcon().or_service_specific_exception(-1)?;
        check_host_service_permission(
            &caller_secontext,
            &[name.to_string()],
            vm.requester_debug_pid,
            vm.requester_uid,
        )
        .with_log()
        .or_binder_exception(ExceptionCode::SECURITY)?;

        // If we've previously proxied this service, we only need to return the connection info
        if let Some(p) = self.proxied_services.lock().unwrap().get(name) {
            return Ok(ServiceConnectionInfo::Vsock(p.clone()));
        }

        let info = host_services::get_service_connection_info(name, cid)?;
        self.proxied_services.lock().unwrap().insert(name.to_string(), info.clone());
        Ok(ServiceConnectionInfo::Vsock(info.clone()))
    }
}

/// Implementation of `IVirtualMachineService`, the entry point of the AIDL service.
struct VirtualMachineService {
    vm_instance: Arc<VmInstance>,
}

impl Interface for VirtualMachineService {}

impl aidl::IVirtualMachineService for VirtualMachineService {
    fn notifyPayloadStarted(&self) -> binder::Result<()> {
        let vm = &self.vm_instance;
        let cid = vm.cid;
        info!("VM with CID {cid} started payload");
        vm.update_payload_state(PayloadState::Started)
            .or_binder_exception(ExceptionCode::ILLEGAL_STATE)?;
        vm.callbacks.notify_payload_started(cid);

        let vm_start_timestamp = vm.vm_metric.lock().unwrap().start_timestamp;
        write_vm_booted_stats(vm.requester_uid as i32, &vm.name, vm_start_timestamp);
        Ok(())
    }

    fn notifyPayloadReady(&self) -> binder::Result<()> {
        let vm = &self.vm_instance;
        let cid = vm.cid;
        info!("VM with CID {cid} reported payload is ready");
        vm.update_payload_state(PayloadState::Ready)
            .or_binder_exception(ExceptionCode::ILLEGAL_STATE)?;
        vm.callbacks.notify_payload_ready(cid);
        Ok(())
    }

    fn notifyPayloadFinished(&self, exit_code: i32) -> binder::Result<()> {
        let vm = &self.vm_instance;
        let cid = vm.cid;
        info!("VM with CID {cid} finished payload");
        vm.update_payload_state(PayloadState::Finished)
            .or_binder_exception(ExceptionCode::ILLEGAL_STATE)?;
        vm.callbacks.notify_payload_finished(cid, exit_code);
        Ok(())
    }

    fn notifyError(&self, error_code: aidl::ErrorCode, message: &str) -> binder::Result<()> {
        let vm = &self.vm_instance;
        let cid = vm.cid;
        info!("VM with CID {cid} encountered an error: {error_code:?} message: {message}");
        vm.update_payload_state(PayloadState::Finished)
            .or_binder_exception(ExceptionCode::ILLEGAL_STATE)?;
        vm.callbacks.notify_error(cid, error_code, message);
        Ok(())
    }

    fn getSecretkeeper(&self) -> binder::Result<Strong<dyn aidl::ISecretkeeper>> {
        if !secretkeeper::is_supported() {
            return Err(StatusCode::NAME_NOT_FOUND)?;
        }
        let sk = binder::wait_for_interface(SECRETKEEPER_IDENTIFIER)?;
        Ok(aidl::BnSecretkeeper::new_binder(SecretkeeperProxy(sk), BinderFeatures::default()))
    }

    fn requestAttestation(
        &self,
        csr: &[u8],
        test_mode: bool,
    ) -> binder::Result<Vec<aidl::Certificate>> {
        if let Some(service) = global_service() {
            service.requestAttestation(csr, get_calling_uid() as i32, test_mode)
        } else {
            Err(anyhow!("early VMs doesn't support requestAttestation"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }

    fn claimSecretkeeperEntry(&self, id: &[u8; 64]) -> binder::Result<()> {
        if let Some(service) = global_service() {
            service.claimSecretkeeperEntry(id)
        } else {
            Err(anyhow!("early VMs doesn't support claimSecretkeeperEntry"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }

    fn getHostRpcProvider(&self) -> binder::Result<Strong<dyn IRpcProvider>> {
        Ok(BnRpcProvider::new_binder(
            HostRpcProvider {
                vm_instance: self.vm_instance.clone(),
                proxied_services: Default::default(),
            },
            BinderFeatures::default(),
        ))
    }

    fn registerGuestAgent(
        &self,
        guest_agent: &Strong<dyn aidl::IGuestAgent>,
    ) -> binder::Result<()> {
        let vm = &self.vm_instance;
        let cid = vm.cid;
        info!("VM with CID {cid} has registered guest agent");
        let wrapper = GuestAgentWrapper::new(guest_agent);
        let wrapped_agent = aidl::BnGuestAgent::new_binder(wrapper, BinderFeatures::default());
        vm.set_guest_agent(&wrapped_agent);
        vm.callbacks.notify_guest_agent_registered(cid, &wrapped_agent);
        Ok(())
    }

    fn getEncryptedStoreKEK(&self) -> binder::Result<Option<Strong<dyn aidl::IEncryptedStoreKEK>>> {
        let vm = &self.vm_instance;
        if let Some(encrypted_store_kek) = &vm.encrypted_store_kek {
            let kek_wrapper = aidl::BnEncryptedStoreKEK::new_binder(
                EncryptedStoreKEKWrapper::new(encrypted_store_kek),
                BinderFeatures::default(),
            );
            Ok(Some(kek_wrapper))
        } else {
            Ok(None)
        }
    }

    fn forwardAtom(&self, atom: &aidl::Atom) -> binder::Result<()> {
        let vm = &self.vm_instance;
        let Some(service) = global_service() else {
            return Err(anyhow!("early VMs don't support forwardAtom"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        };
        service.forwardAtom(atom, vm.requester_uid.try_into().unwrap(), &vm.name)
    }
}

// Unfortunately it looks like we can't pass the IEncryptedStoreKEK object we got from the app all
// the way to microdroid_manager, as calling functions on it fails with the "Cannot send binder
// from unrelated binder RPC session" error. Hence we create another IEncryptedStoreKEK that wraps
// the original one, and pass the wrapper to the microdroid_manager.
struct EncryptedStoreKEKWrapper {
    wrapped_kek: Strong<dyn aidl::IEncryptedStoreKEK>,
}

impl EncryptedStoreKEKWrapper {
    fn new(kek: &Strong<dyn aidl::IEncryptedStoreKEK>) -> Self {
        Self { wrapped_kek: kek.clone() }
    }
}

impl Interface for EncryptedStoreKEKWrapper {}

impl aidl::IEncryptedStoreKEK for EncryptedStoreKEKWrapper {
    fn getKEK(&self) -> binder::Result<Option<Vec<u8>>> {
        self.wrapped_kek.getKEK()
    }

    fn onKEKCreated(&self, kek: &[u8]) -> binder::Result<()> {
        self.wrapped_kek.onKEKCreated(kek)
    }
}

// We need a wrapper for ICEStoreWrapper because of the same reason as IEncryptedStoreKEKWrapper.
struct CEStoreKEKWrapper {
    wrapped_kek: Strong<dyn aidl::ICEStoreKEK>,
}

impl CEStoreKEKWrapper {
    fn new(kek: &Strong<dyn aidl::ICEStoreKEK>) -> Self {
        Self { wrapped_kek: kek.clone() }
    }
}

impl Interface for CEStoreKEKWrapper {}

impl aidl::ICEStoreKEK for CEStoreKEKWrapper {
    fn getKEK(&self) -> binder::Result<Option<Vec<u8>>> {
        self.wrapped_kek.getKEK()
    }

    fn onKEKCreated(&self, kek: &[u8]) -> binder::Result<()> {
        self.wrapped_kek.onKEKCreated(kek)
    }
}

// We need a wrapper for IGuestAgent because of the same reason as IEncryptedStoreKEKWrapper.
struct GuestAgentWrapper {
    wrapped: Strong<dyn aidl::IGuestAgent>,
}

impl GuestAgentWrapper {
    fn new(agent: &Strong<dyn aidl::IGuestAgent>) -> Self {
        Self { wrapped: agent.clone() }
    }
}

impl Interface for GuestAgentWrapper {}

impl aidl::IGuestAgent for GuestAgentWrapper {
    fn shutdownAsync(&self) -> binder::Result<()> {
        self.wrapped.shutdownAsync()
    }

    fn startDumpVsockServer(&self, args: &[String]) -> binder::Result<i32> {
        self.wrapped.startDumpVsockServer(args)
    }

    fn startOrStopTracedRelayService(&self, start: bool) -> binder::Result<()> {
        self.wrapped.startOrStopTracedRelayService(start)
    }

    fn trimAsync(&self) -> binder::Result<()> {
        self.wrapped.trimAsync()
    }

    fn userUnlocked(&self, user_id: i32, kek: &Strong<dyn ICEStoreKEK>) -> binder::Result<()> {
        let kek_wrapper =
            aidl::BnCEStoreKEK::new_binder(CEStoreKEKWrapper::new(kek), BinderFeatures::default());
        self.wrapped.userUnlocked(user_id, &kek_wrapper)
    }
}

fn is_vm_capabilities_hal_supported() -> bool {
    binder::is_declared(VM_CAPABILITIES_HAL_IDENTIFIER)
        .expect("failed to check if {VM_CAPABILITIES_HAL_IDENTIFIER} is present")
}

impl VirtualMachineService {
    fn new_binder(vm_instance: Arc<VmInstance>) -> Strong<dyn aidl::IVirtualMachineService> {
        aidl::BnVirtualMachineService::new_binder(
            VirtualMachineService { vm_instance },
            BinderFeatures::default(),
        )
    }
}

// KEEP IN SYNC WITH early_vms.xsd
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct EarlyVm {
    name: String,
    cid: i32,
    path: String,
}

#[derive(Debug, Default, Deserialize)]
struct EarlyVms {
    early_vm: Vec<EarlyVm>,
}

impl EarlyVm {
    /// Verifies that the provided executable path matches the expected path stored in the XML
    /// configuration.
    /// If the provided path starts with `/system`, it will be stripped before comparison.
    fn check_exe_paths_match<P: AsRef<Path>>(&self, calling_exe_path: P) -> binder::Result<()> {
        let actual_path = calling_exe_path.as_ref();
        if Path::new(&self.path)
            == Path::new("/").join(actual_path.strip_prefix("/system").unwrap_or(actual_path))
        {
            return Ok(());
        }
        Err(Status::new_service_specific_error_str(
            -1,
            Some(format!(
                "Early VM '{}' executable paths do not match. Expected: {}. Found: {:?}.",
                self.name,
                self.path,
                actual_path.display()
            )),
        ))
    }
}

static EARLY_VMS_CACHE: LazyLock<Mutex<HashMap<CallingPartition, Vec<EarlyVm>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn range_for_partition(partition: CallingPartition) -> Range<Cid> {
    match partition {
        CallingPartition::System => 100..200,
        CallingPartition::SystemExt | CallingPartition::Product => 200..300,
        CallingPartition::Vendor | CallingPartition::Odm => 300..400,
        CallingPartition::Unknown => 0..0,
    }
}

fn get_early_vms_in_path(xml_path: &Path) -> Result<Vec<EarlyVm>> {
    if !xml_path.exists() {
        bail!("{} doesn't exist", xml_path.display());
    }

    let xml =
        fs::read(xml_path).with_context(|| format!("Failed to read {}", xml_path.display()))?;
    let xml = String::from_utf8(xml)
        .with_context(|| format!("{} is not a valid UTF-8 file", xml_path.display()))?;
    let early_vms: EarlyVms = serde_xml_rs::from_str(&xml)
        .with_context(|| format!("Can't parse {}", xml_path.display()))?;

    Ok(early_vms.early_vm)
}

fn validate_cid_range(early_vms: &[EarlyVm], cid_range: &Range<Cid>) -> Result<()> {
    for early_vm in early_vms {
        let cid = early_vm
            .cid
            .try_into()
            .with_context(|| format!("VM '{}' uses Invalid CID {}", early_vm.name, early_vm.cid))?;

        ensure!(
            cid_range.contains(&cid),
            "VM '{}' uses CID {cid} which is out of range. Available CIDs: {cid_range:?}",
            early_vm.name
        );
    }
    Ok(())
}

fn get_early_vms_in_partition(partition: CallingPartition) -> Result<Vec<EarlyVm>> {
    let mut cache = EARLY_VMS_CACHE.lock().unwrap();

    if let Some(result) = cache.get(&partition) {
        return Ok(result.clone());
    }

    let pattern = format!("/{partition}/etc/avf/early_vms*.xml");
    let mut early_vms = Vec::new();
    for entry in glob::glob(&pattern).with_context(|| format!("Failed to glob {}", &pattern))? {
        match entry {
            Ok(path) => early_vms.extend(get_early_vms_in_path(&path)?),
            Err(e) => error!("Error while globbing (but continuing) {}: {}", &pattern, e),
        }
    }

    validate_cid_range(&early_vms, &range_for_partition(partition))
        .with_context(|| format!("CID validation for {partition} failed"))?;

    cache.insert(partition, early_vms.clone());

    Ok(early_vms)
}

fn find_early_vm<'a>(early_vms: &'a [EarlyVm], name: &str) -> Result<&'a EarlyVm> {
    let mut found_vm: Option<&EarlyVm> = None;

    for early_vm in early_vms {
        if early_vm.name != name {
            continue;
        }

        if found_vm.is_some() {
            bail!("Multiple VMs named '{name}' are found");
        }

        found_vm = Some(early_vm);
    }

    found_vm.ok_or_else(|| anyhow!("Can't find a VM named '{name}'"))
}

fn find_early_vm_for_partition(partition: CallingPartition, name: &str) -> Result<EarlyVm> {
    let early_vms = get_early_vms_in_partition(partition)
        .with_context(|| format!("Failed to get early VMs from {partition}"))?;

    Ok(find_early_vm(&early_vms, name)
        .with_context(|| format!("Failed to find early VM '{name}' in {partition}"))?
        .clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_allowed_label_for_partition() -> Result<()> {
        let expected_results = vec![
            (CallingPartition::System, "u:object_r:system_file:s0", true),
            (CallingPartition::System, "u:object_r:apk_data_file:s0", true),
            (CallingPartition::System, "u:object_r:app_data_file:s0", false),
            (CallingPartition::System, "u:object_r:app_data_file:s0:c512,c768", false),
            (CallingPartition::System, "u:object_r:privapp_data_file:s0:c512,c768", false),
            (CallingPartition::System, "invalid", false),
            (CallingPartition::System, "user:role:apk_data_file:severity:categories", true),
            (
                CallingPartition::System,
                "user:role:apk_data_file:severity:categories:extraneous",
                false,
            ),
            (CallingPartition::System, "u:object_r:vendor_unknowable:s0", false),
            (CallingPartition::Vendor, "u:object_r:vendor_unknowable:s0", true),
        ];

        for (calling_partition, label, expected_valid) in expected_results {
            let context = SeContext::new(label)?;
            let result = check_label_is_allowed(&context, calling_partition);
            if expected_valid != result.is_ok() {
                bail!(
                    "Expected label {label} to be {} for {calling_partition} partition",
                    if expected_valid { "allowed" } else { "disallowed" }
                );
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
        #[allow(clippy::unbuffered_bytes)]
        for (b1, b2) in idsig_empty.bytes().zip(idsig_invalid.bytes()) {
            assert!(b1.unwrap() == b2.unwrap(), "idsig files differ")
        }
        Ok(())
    }
    #[test]
    fn test_append_kernel_param_first_param() {
        let mut vm_config = aidl::VirtualMachineRawConfig { ..Default::default() };
        append_kernel_param("foo=1", &mut vm_config);
        assert_eq!(vm_config.params, Some("foo=1".to_owned()))
    }

    #[test]
    fn test_append_kernel_param() {
        let mut vm_config = aidl::VirtualMachineRawConfig {
            params: Some("foo=5".to_owned()),
            ..Default::default()
        };
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
    fn test_extract_os_name_from_microdroid_16k_config() -> Result<()> {
        test_extract_os_name_from_config_path(
            Path::new("/apex/com.android.virt/etc/microdroid_16k.json"),
            Some("microdroid_16k"),
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
        std::fs::write(tmp_dir_path.join("microdroid.img"), b"")?;
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
                <name>vm_demo_native_early_2</name>
                <cid>456</cid>
                <path>/system/bin/vm_demo_native_early_2</path>
            </early_vm>
        </early_vms>
        "#,
        )?;

        let cid_range = 100..1000;

        let early_vms = get_early_vms_in_path(&xml_path)?;
        validate_cid_range(&early_vms, &cid_range)?;

        let test_cases = [
            (
                "vm_demo_native_early",
                EarlyVm {
                    name: "vm_demo_native_early".to_owned(),
                    cid: 123,
                    path: "/system/bin/vm_demo_native_early".to_owned(),
                },
            ),
            (
                "vm_demo_native_early_2",
                EarlyVm {
                    name: "vm_demo_native_early_2".to_owned(),
                    cid: 456,
                    path: "/system/bin/vm_demo_native_early_2".to_owned(),
                },
            ),
        ];

        for (name, expected) in test_cases {
            let result = find_early_vm(&early_vms, name)?;
            assert_eq!(result, &expected);
        }

        Ok(())
    }

    #[test]
    fn test_invalid_cid_validation() -> Result<()> {
        let tmp_dir = tempfile::TempDir::new()?;
        let xml_path = tmp_dir.path().join("early_vms.xml");

        let cid_range = 100..1000;

        for cid in [-1, 999999] {
            std::fs::write(
                &xml_path,
                format!(
                    r#"<?xml version="1.0" encoding="utf-8"?>
        <early_vms>
            <early_vm>
                <name>vm_demo_invalid_cid</name>
                <cid>{cid}</cid>
                <path>/system/bin/vm_demo_invalid_cid</path>
            </early_vm>
        </early_vms>
        "#
                ),
            )?;

            let early_vms = get_early_vms_in_path(&xml_path)?;
            assert!(validate_cid_range(&early_vms, &cid_range).is_err(), "should fail");
        }

        Ok(())
    }

    #[test]
    fn test_symlink_to_system_ext_supported() -> Result<()> {
        let link_path = Path::new("/system/system_ext/file");
        let partition = find_partition(Some(link_path)).unwrap();
        assert_eq!(CallingPartition::SystemExt, partition);
        Ok(())
    }

    #[test]
    fn test_symlink_to_product_supported() -> Result<()> {
        let link_path = Path::new("/system/product/file");
        let partition = find_partition(Some(link_path)).unwrap();
        assert_eq!(CallingPartition::Product, partition);
        Ok(())
    }

    #[test]
    fn test_vendor_in_data() {
        assert_eq!(
            CallingPartition::Vendor,
            find_partition(Some(Path::new("/data/nativetest64/vendor/file"))).unwrap()
        );
    }

    #[test]
    fn early_vm_exe_paths_match_succeeds_with_same_paths() {
        let early_vm = EarlyVm {
            name: "vm_demo_native_early".to_owned(),
            cid: 123,
            path: "/system_ext/bin/vm_demo_native_early".to_owned(),
        };
        let calling_exe_path = "/system_ext/bin/vm_demo_native_early";
        assert!(early_vm.check_exe_paths_match(calling_exe_path).is_ok())
    }

    #[test]
    fn early_vm_exe_paths_match_succeeds_with_calling_exe_path_from_system() {
        let early_vm = EarlyVm {
            name: "vm_demo_native_early".to_owned(),
            cid: 123,
            path: "/system_ext/bin/vm_demo_native_early".to_owned(),
        };
        let calling_exe_path = "/system/system_ext/bin/vm_demo_native_early";
        assert!(early_vm.check_exe_paths_match(calling_exe_path).is_ok())
    }

    #[test]
    fn early_vm_exe_paths_match_fails_with_unmatched_paths() {
        let early_vm = EarlyVm {
            name: "vm_demo_native_early".to_owned(),
            cid: 123,
            path: "/system_ext/bin/vm_demo_native_early".to_owned(),
        };
        let calling_exe_path = "/system/etc/system_ext/bin/vm_demo_native_early";
        assert!(early_vm.check_exe_paths_match(calling_exe_path).is_err())
    }

    #[test]
    fn test_duplicated_early_vms() -> Result<()> {
        let tmp_dir = tempfile::TempDir::new()?;
        let tmp_dir_path = tmp_dir.path().to_owned();
        let xml_path = tmp_dir_path.join("early_vms.xml");

        std::fs::write(
            &xml_path,
            br#"<?xml version="1.0" encoding="utf-8"?>
        <early_vms>
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
        </early_vms>
        "#,
        )?;

        let cid_range = 100..1000;

        let early_vms = get_early_vms_in_path(&xml_path)?;
        validate_cid_range(&early_vms, &cid_range)?;

        assert!(find_early_vm(&early_vms, "vm_demo_duplicated_name").is_err(), "should fail");

        Ok(())
    }
}
