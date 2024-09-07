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

use crate::atom::{forward_vm_booted_atom, forward_vm_creation_atom, forward_vm_exited_atom};
use crate::maintenance;
use crate::remote_provisioning;
use crate::rkpvm::{generate_ecdsa_p256_key_pair, request_attestation};
use crate::{get_calling_pid, get_calling_uid, REMOTELY_PROVISIONED_COMPONENT_SERVICE_NAME};
use android_os_permissions_aidl::aidl::android::os::IPermissionController;
use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon;
use android_system_virtualizationmaintenance::aidl::android::system::virtualizationmaintenance;
use android_system_virtualizationservice::aidl::android::system::virtualizationservice;
use android_system_virtualizationservice_internal as android_vs_internal;
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice;
use android_system_vmtethering::aidl::android::system::vmtethering;
use android_vs_internal::aidl::android::system::virtualizationservice_internal;
use anyhow::{anyhow, ensure, Context, Result};
use avflog::LogResult;
use binder::{
    self, wait_for_interface, BinderFeatures, ExceptionCode, Interface, IntoBinderResult,
    LazyServiceGuard, ParcelFileDescriptor, Status, Strong,
};
use libc::{VMADDR_CID_HOST, VMADDR_CID_HYPERVISOR, VMADDR_CID_LOCAL};
use log::{error, info, warn};
use nix::unistd::{chown, Uid};
use openssl::x509::X509;
use rand::Fill;
use rkpd_client::get_rkpd_attestation_key;
use rustutils::{
    system_properties,
    users::{multiuser_get_app_id, multiuser_get_user_id},
};
use serde::Deserialize;
use service_vm_comm::Response;
use std::collections::{HashMap, HashSet};
use std::fs::{self, create_dir, remove_dir_all, remove_file, set_permissions, File, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::raw::{pid_t, uid_t};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Weak};
use tombstoned_client::{DebuggerdDumpType, TombstonedConnection};
use virtualizationcommon::Certificate::Certificate;
use virtualizationmaintenance::{
    IVirtualizationMaintenance::IVirtualizationMaintenance,
    IVirtualizationReconciliationCallback::IVirtualizationReconciliationCallback,
};
use virtualizationservice::{
    AssignableDevice::AssignableDevice, VirtualMachineDebugInfo::VirtualMachineDebugInfo,
};
use virtualizationservice_internal::{
    AtomVmBooted::AtomVmBooted,
    AtomVmCreationRequested::AtomVmCreationRequested,
    AtomVmExited::AtomVmExited,
    IBoundDevice::IBoundDevice,
    IGlobalVmContext::{BnGlobalVmContext, IGlobalVmContext},
    IVfioHandler::VfioDev::VfioDev,
    IVfioHandler::{BpVfioHandler, IVfioHandler},
    IVirtualizationServiceInternal::IVirtualizationServiceInternal,
    IVmnic::{BpVmnic, IVmnic},
};
use virtualmachineservice::IVirtualMachineService::VM_TOMBSTONES_SERVICE_PORT;
use vmtethering::IVmTethering::{BpVmTethering, IVmTethering};
use vsock::{VsockListener, VsockStream};

/// The unique ID of a VM used (together with a port number) for vsock communication.
pub type Cid = u32;

/// Directory in which to write disk image files used while running VMs.
pub const TEMPORARY_DIRECTORY: &str = "/data/misc/virtualizationservice";

/// The first CID to assign to a guest VM managed by the VirtualizationService. CIDs lower than this
/// are reserved for the host or other usage.
const GUEST_CID_MIN: Cid = 2048;
const GUEST_CID_MAX: Cid = 65535;

const SYSPROP_LAST_CID: &str = "virtualizationservice.state.last_cid";

const CHUNK_RECV_MAX_LEN: usize = 1024;

/// The fake certificate is used for testing only when a client VM requests attestation in test
/// mode, it is a single certificate extracted on an unregistered device for testing.
/// Here is the snapshot of the certificate:
///
/// ```
/// Certificate:
/// Data:
/// Version: 3 (0x2)
/// Serial Number:
///     59:ae:50:98:95:e1:34:25:f1:21:93:c0:4c:e5:24:66
/// Signature Algorithm: ecdsa-with-SHA256
/// Issuer: CN = Droid Unregistered Device CA, O = Google Test LLC
/// Validity
///     Not Before: Feb  5 14:39:39 2024 GMT
///     Not After : Feb 14 14:39:39 2024 GMT
/// Subject: CN = 59ae509895e13425f12193c04ce52466, O = TEE
/// Subject Public Key Info:
///     Public Key Algorithm: id-ecPublicKey
///         Public-Key: (256 bit)
///         pub:
///             04:30:32:cd:95:12:b0:71:8b:b7:14:44:26:58:d5:
///             82:8c:25:55:2c:6d:ef:98:e3:4f:88:d0:74:82:09:
///             3e:8d:6c:f0:f2:18:d5:83:0e:0d:f2:ce:c5:15:38:
///             e5:6a:e6:4d:4d:95:15:b7:24:e7:cb:4b:63:42:21:
///             bc:36:c6:0a:d8
///         ASN1 OID: prime256v1
///         NIST CURVE: P-256
/// X509v3 extensions:
///  ...
/// ```
const FAKE_CERTIFICATE_FOR_TESTING: &[u8] = &[
    0x30, 0x82, 0x01, 0xee, 0x30, 0x82, 0x01, 0x94, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x10, 0x59,
    0xae, 0x50, 0x98, 0x95, 0xe1, 0x34, 0x25, 0xf1, 0x21, 0x93, 0xc0, 0x4c, 0xe5, 0x24, 0x66, 0x30,
    0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02, 0x30, 0x41, 0x31, 0x25, 0x30,
    0x23, 0x06, 0x03, 0x55, 0x04, 0x03, 0x13, 0x1c, 0x44, 0x72, 0x6f, 0x69, 0x64, 0x20, 0x55, 0x6e,
    0x72, 0x65, 0x67, 0x69, 0x73, 0x74, 0x65, 0x72, 0x65, 0x64, 0x20, 0x44, 0x65, 0x76, 0x69, 0x63,
    0x65, 0x20, 0x43, 0x41, 0x31, 0x18, 0x30, 0x16, 0x06, 0x03, 0x55, 0x04, 0x0a, 0x13, 0x0f, 0x47,
    0x6f, 0x6f, 0x67, 0x6c, 0x65, 0x20, 0x54, 0x65, 0x73, 0x74, 0x20, 0x4c, 0x4c, 0x43, 0x30, 0x1e,
    0x17, 0x0d, 0x32, 0x34, 0x30, 0x32, 0x30, 0x35, 0x31, 0x34, 0x33, 0x39, 0x33, 0x39, 0x5a, 0x17,
    0x0d, 0x32, 0x34, 0x30, 0x32, 0x31, 0x34, 0x31, 0x34, 0x33, 0x39, 0x33, 0x39, 0x5a, 0x30, 0x39,
    0x31, 0x29, 0x30, 0x27, 0x06, 0x03, 0x55, 0x04, 0x03, 0x13, 0x20, 0x35, 0x39, 0x61, 0x65, 0x35,
    0x30, 0x39, 0x38, 0x39, 0x35, 0x65, 0x31, 0x33, 0x34, 0x32, 0x35, 0x66, 0x31, 0x32, 0x31, 0x39,
    0x33, 0x63, 0x30, 0x34, 0x63, 0x65, 0x35, 0x32, 0x34, 0x36, 0x36, 0x31, 0x0c, 0x30, 0x0a, 0x06,
    0x03, 0x55, 0x04, 0x0a, 0x13, 0x03, 0x54, 0x45, 0x45, 0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07,
    0x03, 0x42, 0x00, 0x04, 0x30, 0x32, 0xcd, 0x95, 0x12, 0xb0, 0x71, 0x8b, 0xb7, 0x14, 0x44, 0x26,
    0x58, 0xd5, 0x82, 0x8c, 0x25, 0x55, 0x2c, 0x6d, 0xef, 0x98, 0xe3, 0x4f, 0x88, 0xd0, 0x74, 0x82,
    0x09, 0x3e, 0x8d, 0x6c, 0xf0, 0xf2, 0x18, 0xd5, 0x83, 0x0e, 0x0d, 0xf2, 0xce, 0xc5, 0x15, 0x38,
    0xe5, 0x6a, 0xe6, 0x4d, 0x4d, 0x95, 0x15, 0xb7, 0x24, 0xe7, 0xcb, 0x4b, 0x63, 0x42, 0x21, 0xbc,
    0x36, 0xc6, 0x0a, 0xd8, 0xa3, 0x76, 0x30, 0x74, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x1d, 0x0e, 0x04,
    0x16, 0x04, 0x14, 0x39, 0x81, 0x41, 0x0a, 0xb9, 0xf3, 0xf4, 0x5b, 0x75, 0x97, 0x4a, 0x46, 0xd6,
    0x30, 0x9e, 0x1d, 0x7a, 0x3b, 0xec, 0xa8, 0x30, 0x1f, 0x06, 0x03, 0x55, 0x1d, 0x23, 0x04, 0x18,
    0x30, 0x16, 0x80, 0x14, 0x82, 0xbd, 0x00, 0xde, 0xcb, 0xc5, 0xe7, 0x72, 0x87, 0x3d, 0x1c, 0x0a,
    0x1e, 0x78, 0x4f, 0xf5, 0xd3, 0xc1, 0x3e, 0xb8, 0x30, 0x0f, 0x06, 0x03, 0x55, 0x1d, 0x13, 0x01,
    0x01, 0xff, 0x04, 0x05, 0x30, 0x03, 0x01, 0x01, 0xff, 0x30, 0x0e, 0x06, 0x03, 0x55, 0x1d, 0x0f,
    0x01, 0x01, 0xff, 0x04, 0x04, 0x03, 0x02, 0x02, 0x04, 0x30, 0x11, 0x06, 0x0a, 0x2b, 0x06, 0x01,
    0x04, 0x01, 0xd6, 0x79, 0x02, 0x01, 0x1e, 0x04, 0x03, 0xa1, 0x01, 0x08, 0x30, 0x0a, 0x06, 0x08,
    0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02, 0x03, 0x48, 0x00, 0x30, 0x45, 0x02, 0x21, 0x00,
    0xae, 0xd8, 0x40, 0x9e, 0x37, 0x3e, 0x5c, 0x9c, 0xe2, 0x93, 0x3d, 0x8c, 0xf7, 0x05, 0x10, 0xe7,
    0xd1, 0x2b, 0x87, 0x8a, 0xee, 0xd6, 0x1e, 0x6c, 0x3b, 0xd2, 0x91, 0x3e, 0xa5, 0xdf, 0x91, 0x20,
    0x02, 0x20, 0x7f, 0x0f, 0x29, 0x54, 0x60, 0x80, 0x07, 0x50, 0x5f, 0x56, 0x6b, 0x9f, 0xe0, 0x94,
    0xb4, 0x3f, 0x3b, 0x0f, 0x61, 0xa0, 0x33, 0x40, 0xe6, 0x1a, 0x42, 0xda, 0x4b, 0xa4, 0xfd, 0x92,
    0xb9, 0x0f,
];

static FAKE_PROVISIONED_KEY_BLOB_FOR_TESTING: Mutex<Option<Vec<u8>>> = Mutex::new(None);
static VFIO_SERVICE: LazyLock<Strong<dyn IVfioHandler>> = LazyLock::new(|| {
    wait_for_interface(<BpVfioHandler as IVfioHandler>::get_descriptor())
        .expect("Could not connect to VfioHandler")
});
static NETWORK_SERVICE: LazyLock<Strong<dyn IVmnic>> = LazyLock::new(|| {
    wait_for_interface(<BpVmnic as IVmnic>::get_descriptor()).expect("Could not connect to Vmnic")
});
static TETHERING_SERVICE: LazyLock<Strong<dyn IVmTethering>> = LazyLock::new(|| {
    wait_for_interface(<BpVmTethering as IVmTethering>::get_descriptor())
        .expect("Could not connect to VmTethering")
});

fn is_valid_guest_cid(cid: Cid) -> bool {
    (GUEST_CID_MIN..=GUEST_CID_MAX).contains(&cid)
}

/// Singleton service for allocating globally-unique VM resources, such as the CID, and running
/// singleton servers, like tombstone receiver.
#[derive(Clone)]
pub struct VirtualizationServiceInternal {
    state: Arc<Mutex<GlobalState>>,
    display_service_set: Arc<Condvar>,
}

impl VirtualizationServiceInternal {
    pub fn init() -> VirtualizationServiceInternal {
        let service = VirtualizationServiceInternal {
            state: Arc::new(Mutex::new(GlobalState::new())),
            display_service_set: Arc::new(Condvar::new()),
        };

        std::thread::spawn(|| {
            if let Err(e) = handle_stream_connection_tombstoned() {
                warn!("Error receiving tombstone from guest or writing them. Error: {:?}", e);
            }
        });

        service
    }
}

impl Interface for VirtualizationServiceInternal {}

impl IVirtualizationServiceInternal for VirtualizationServiceInternal {
    fn setDisplayService(
        &self,
        ibinder: &binder::SpIBinder,
    ) -> std::result::Result<(), binder::Status> {
        check_manage_access()?;
        check_use_custom_virtual_machine()?;
        let state = &mut *self.state.lock().unwrap();
        state.display_service = Some(ibinder.clone());
        self.display_service_set.notify_all();
        Ok(())
    }

    fn clearDisplayService(&self) -> std::result::Result<(), binder::Status> {
        check_manage_access()?;
        check_use_custom_virtual_machine()?;
        let state = &mut *self.state.lock().unwrap();
        state.display_service = None;
        self.display_service_set.notify_all();
        Ok(())
    }

    fn waitDisplayService(&self) -> std::result::Result<binder::SpIBinder, binder::Status> {
        check_manage_access()?;
        check_use_custom_virtual_machine()?;
        let state = self
            .display_service_set
            .wait_while(self.state.lock().unwrap(), |state| state.display_service.is_none())
            .unwrap();
        Ok((state.display_service)
            .as_ref()
            .cloned()
            .expect("Display service cannot be None in this context"))
    }
    fn removeMemlockRlimit(&self) -> binder::Result<()> {
        let pid = get_calling_pid();
        let lim = libc::rlimit { rlim_cur: libc::RLIM_INFINITY, rlim_max: libc::RLIM_INFINITY };

        // SAFETY: borrowing the new limit struct only
        let ret = unsafe { libc::prlimit(pid, libc::RLIMIT_MEMLOCK, &lim, std::ptr::null_mut()) };

        match ret {
            0 => Ok(()),
            -1 => Err(std::io::Error::last_os_error().into()),
            n => Err(anyhow!("Unexpected return value from prlimit(): {n}")),
        }
        .or_binder_exception(ExceptionCode::ILLEGAL_STATE)
    }

    fn allocateGlobalVmContext(
        &self,
        requester_debug_pid: i32,
    ) -> binder::Result<Strong<dyn IGlobalVmContext>> {
        check_manage_access()?;

        let requester_uid = get_calling_uid();
        let requester_debug_pid = requester_debug_pid as pid_t;
        let state = &mut *self.state.lock().unwrap();
        state
            .allocate_vm_context(requester_uid, requester_debug_pid)
            .or_binder_exception(ExceptionCode::ILLEGAL_STATE)
    }

    fn atomVmBooted(&self, atom: &AtomVmBooted) -> Result<(), Status> {
        forward_vm_booted_atom(atom);
        Ok(())
    }

    fn atomVmCreationRequested(&self, atom: &AtomVmCreationRequested) -> Result<(), Status> {
        forward_vm_creation_atom(atom);
        Ok(())
    }

    fn atomVmExited(&self, atom: &AtomVmExited) -> Result<(), Status> {
        forward_vm_exited_atom(atom);
        Ok(())
    }

    fn debugListVms(&self) -> binder::Result<Vec<VirtualMachineDebugInfo>> {
        check_debug_access()?;

        let state = &mut *self.state.lock().unwrap();
        let cids = state
            .held_contexts
            .iter()
            .filter_map(|(_, inst)| Weak::upgrade(inst))
            .map(|vm| {
                let vm = vm.lock().unwrap();
                VirtualMachineDebugInfo {
                    cid: vm.cid as i32,
                    temporaryDirectory: vm.get_temp_dir().to_string_lossy().to_string(),
                    requesterUid: vm.requester_uid as i32,
                    requesterPid: vm.requester_debug_pid,
                    hostConsoleName: vm.host_console_name.clone(),
                }
            })
            .collect();
        Ok(cids)
    }

    fn enableTestAttestation(&self) -> binder::Result<()> {
        check_manage_access()?;
        check_use_custom_virtual_machine()?;
        if !cfg!(remote_attestation) {
            return Err(Status::new_exception_str(
                ExceptionCode::UNSUPPORTED_OPERATION,
                Some(
                    "enableTestAttestation is not supported with the remote_attestation \
                     feature disabled",
                ),
            ))
            .with_log();
        }
        let res = generate_ecdsa_p256_key_pair()
            .context("Failed to generate ECDSA P-256 key pair for testing")
            .with_log()
            .or_service_specific_exception(-1)?;
        // Wait until the service VM shuts down, so that the Service VM will be restarted when
        // the key generated in the current session will be used for attestation.
        // This ensures that different Service VM sessions have the same KEK for the key blob.
        service_vm_manager::wait_until_service_vm_shuts_down()
            .context("Failed to wait until the service VM shuts down")
            .with_log()
            .or_service_specific_exception(-1)?;
        match res {
            Response::GenerateEcdsaP256KeyPair(key_pair) => {
                FAKE_PROVISIONED_KEY_BLOB_FOR_TESTING
                    .lock()
                    .unwrap()
                    .replace(key_pair.key_blob.to_vec());
                Ok(())
            }
            _ => Err(remote_provisioning::to_service_specific_error(res)),
        }
        .with_log()
    }

    fn requestAttestation(
        &self,
        csr: &[u8],
        requester_uid: i32,
        test_mode: bool,
    ) -> binder::Result<Vec<Certificate>> {
        check_manage_access()?;
        if !cfg!(remote_attestation) {
            return Err(Status::new_exception_str(
                ExceptionCode::UNSUPPORTED_OPERATION,
                Some(
                    "requestAttestation is not supported with the remote_attestation feature \
                     disabled",
                ),
            ))
            .with_log();
        }
        if !is_remote_provisioning_hal_declared()? {
            return Err(Status::new_exception_str(
                ExceptionCode::UNSUPPORTED_OPERATION,
                Some("AVF remotely provisioned component service is not declared"),
            ))
            .with_log();
        }
        remote_provisioning::check_remote_attestation_is_supported()?;
        info!("Received csr. Requestting attestation...");
        let (key_blob, certificate_chain) = if test_mode {
            check_use_custom_virtual_machine()?;
            info!("Using the fake key blob for testing...");
            (
                FAKE_PROVISIONED_KEY_BLOB_FOR_TESTING
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| anyhow!("No key blob for testing"))
                    .with_log()
                    .or_service_specific_exception(-1)?,
                FAKE_CERTIFICATE_FOR_TESTING.to_vec(),
            )
        } else {
            info!("Retrieving the remotely provisioned keys from RKPD...");
            let attestation_key = get_rkpd_attestation_key(
                REMOTELY_PROVISIONED_COMPONENT_SERVICE_NAME,
                requester_uid as u32,
            )
            .context("Failed to retrieve the remotely provisioned keys")
            .with_log()
            .or_service_specific_exception(-1)?;
            (attestation_key.keyBlob, attestation_key.encodedCertChain)
        };
        let mut certificate_chain = split_x509_certificate_chain(&certificate_chain)
            .context("Failed to split the remotely provisioned certificate chain")
            .with_log()
            .or_service_specific_exception(-1)?;
        if certificate_chain.is_empty() {
            return Err(Status::new_service_specific_error_str(
                -1,
                Some("The certificate chain should contain at least 1 certificate"),
            ))
            .with_log();
        }
        let certificate = request_attestation(
            csr.to_vec(),
            key_blob,
            certificate_chain[0].encodedCertificate.clone(),
        )
        .context("Failed to request attestation")
        .with_log()
        .or_service_specific_exception(-1)?;
        certificate_chain.insert(0, Certificate { encodedCertificate: certificate });

        Ok(certificate_chain)
    }

    fn isRemoteAttestationSupported(&self) -> binder::Result<bool> {
        Ok(is_remote_provisioning_hal_declared()?
            && remote_provisioning::is_remote_attestation_supported())
    }

    fn getAssignableDevices(&self) -> binder::Result<Vec<AssignableDevice>> {
        check_use_custom_virtual_machine()?;

        Ok(get_assignable_devices()?
            .device
            .into_iter()
            .map(|x| AssignableDevice { node: x.sysfs_path, dtbo_label: x.dtbo_label })
            .collect::<Vec<_>>())
    }

    fn bindDevicesToVfioDriver(
        &self,
        devices: &[String],
    ) -> binder::Result<Vec<Strong<dyn IBoundDevice>>> {
        check_use_custom_virtual_machine()?;

        let devices = get_assignable_devices()?
            .device
            .into_iter()
            .filter_map(|x| {
                if devices.contains(&x.sysfs_path) {
                    Some(VfioDev { sysfsPath: x.sysfs_path, dtboLabel: x.dtbo_label })
                } else {
                    warn!("device {} is not assignable", x.sysfs_path);
                    None
                }
            })
            .collect::<Vec<VfioDev>>();

        VFIO_SERVICE.bindDevicesToVfioDriver(devices.as_slice())
    }

    fn getDtboFile(&self) -> binder::Result<ParcelFileDescriptor> {
        check_use_custom_virtual_machine()?;

        let state = &mut *self.state.lock().unwrap();
        let file = state.get_dtbo_file().or_service_specific_exception(-1)?;
        Ok(ParcelFileDescriptor::new(file))
    }

    fn allocateInstanceId(&self) -> binder::Result<[u8; 64]> {
        let mut id = [0u8; 64];
        id.try_fill(&mut rand::thread_rng())
            .context("Failed to allocate instance_id")
            .or_service_specific_exception(-1)?;
        let uid = get_calling_uid();
        info!("Allocated a VM's instance_id: {:?}..., for uid: {:?}", &hex::encode(id)[..8], uid);
        let state = &mut *self.state.lock().unwrap();
        if let Some(sk_state) = &mut state.sk_state {
            let user_id = multiuser_get_user_id(uid);
            let app_id = multiuser_get_app_id(uid);
            info!("Recording possible existence of state for (user_id={user_id}, app_id={app_id})");
            if let Err(e) = sk_state.add_id(&id, user_id, app_id) {
                error!("Failed to record the instance_id: {e:?}");
            }
        }

        Ok(id)
    }

    fn removeVmInstance(&self, instance_id: &[u8; 64]) -> binder::Result<()> {
        let state = &mut *self.state.lock().unwrap();
        if let Some(sk_state) = &mut state.sk_state {
            let uid = get_calling_uid();
            info!(
                "Removing a VM's instance_id: {:?}, for uid: {:?}",
                hex::encode(instance_id),
                uid
            );

            let user_id = multiuser_get_user_id(uid);
            let app_id = multiuser_get_app_id(uid);
            sk_state.delete_id(instance_id, user_id, app_id);
        } else {
            info!("ignoring removeVmInstance() as no ISecretkeeper");
        }
        Ok(())
    }

    fn claimVmInstance(&self, instance_id: &[u8; 64]) -> binder::Result<()> {
        let state = &mut *self.state.lock().unwrap();
        if let Some(sk_state) = &mut state.sk_state {
            let uid = get_calling_uid();
            info!(
                "Claiming a VM's instance_id: {:?}, for uid: {:?}",
                hex::encode(instance_id),
                uid
            );

            let user_id = multiuser_get_user_id(uid);
            let app_id = multiuser_get_app_id(uid);
            info!("Recording possible new owner of state for (user_id={user_id}, app_id={app_id})");
            if let Err(e) = sk_state.add_id(instance_id, user_id, app_id) {
                error!("Failed to update the instance_id owner: {e:?}");
            }
        } else {
            info!("ignoring claimVmInstance() as no ISecretkeeper");
        }
        Ok(())
    }

    fn createTapInterface(&self, _iface_name_suffix: &str) -> binder::Result<ParcelFileDescriptor> {
        check_internet_permission()?;
        check_use_custom_virtual_machine()?;
        if !cfg!(network) {
            return Err(Status::new_exception_str(
                ExceptionCode::UNSUPPORTED_OPERATION,
                Some("createTapInterface is not supported with the network feature disabled"),
            ))
            .with_log();
        }
        // TODO(340377643): Use iface_name_suffix after introducing bridge interface, not fixed
        // value.
        let tap_fd = NETWORK_SERVICE.createTapInterface("fixed")?;

        // TODO(340377643): Due to lack of implementation of creating bridge interface, tethering is
        // enabled for TAP interface instead of bridge interface. After introducing creation of
        // bridge interface in AVF, we should modify it.
        TETHERING_SERVICE.enableVmTethering()?;

        Ok(tap_fd)
    }

    fn deleteTapInterface(&self, tap_fd: &ParcelFileDescriptor) -> binder::Result<()> {
        check_internet_permission()?;
        check_use_custom_virtual_machine()?;
        if !cfg!(network) {
            return Err(Status::new_exception_str(
                ExceptionCode::UNSUPPORTED_OPERATION,
                Some("deleteTapInterface is not supported with the network feature disabled"),
            ))
            .with_log();
        }

        // TODO(340377643): Disabling tethering should be for bridge interface, not TAP interface.
        TETHERING_SERVICE.disableVmTethering()?;

        NETWORK_SERVICE.deleteTapInterface(tap_fd)
    }
}

impl IVirtualizationMaintenance for VirtualizationServiceInternal {
    fn appRemoved(&self, user_id: i32, app_id: i32) -> binder::Result<()> {
        let state = &mut *self.state.lock().unwrap();
        if let Some(sk_state) = &mut state.sk_state {
            info!("packageRemoved(user_id={user_id}, app_id={app_id})");
            sk_state.delete_ids_for_app(user_id, app_id).or_service_specific_exception(-1)?;
        } else {
            info!("ignoring packageRemoved(user_id={user_id}, app_id={app_id})");
        }
        Ok(())
    }

    fn userRemoved(&self, user_id: i32) -> binder::Result<()> {
        let state = &mut *self.state.lock().unwrap();
        if let Some(sk_state) = &mut state.sk_state {
            info!("userRemoved({user_id})");
            sk_state.delete_ids_for_user(user_id).or_service_specific_exception(-1)?;
        } else {
            info!("ignoring userRemoved(user_id={user_id})");
        }
        Ok(())
    }

    fn performReconciliation(
        &self,
        callback: &Strong<dyn IVirtualizationReconciliationCallback>,
    ) -> binder::Result<()> {
        let state = &mut *self.state.lock().unwrap();
        if let Some(sk_state) = &mut state.sk_state {
            info!("performReconciliation()");
            sk_state.reconcile(callback).or_service_specific_exception(-1)?;
        } else {
            info!("ignoring performReconciliation()");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct Device {
    dtbo_label: String,
    sysfs_path: String,
}

#[derive(Debug, Default, Deserialize)]
struct Devices {
    device: Vec<Device>,
}

fn get_assignable_devices() -> binder::Result<Devices> {
    let xml_path = Path::new("/vendor/etc/avf/assignable_devices.xml");
    if !xml_path.exists() {
        return Ok(Devices { ..Default::default() });
    }

    let xml = fs::read(xml_path)
        .context("Failed to read assignable_devices.xml")
        .with_log()
        .or_service_specific_exception(-1)?;

    let xml = String::from_utf8(xml)
        .context("assignable_devices.xml is not a valid UTF-8 file")
        .with_log()
        .or_service_specific_exception(-1)?;

    let mut devices: Devices = serde_xml_rs::from_str(&xml)
        .context("can't parse assignable_devices.xml")
        .with_log()
        .or_service_specific_exception(-1)?;

    let mut device_set = HashSet::new();
    devices.device.retain(move |device| {
        if device_set.contains(&device.sysfs_path) {
            warn!("duplicated assignable device {device:?}; ignoring...");
            return false;
        }

        if !Path::new(&device.sysfs_path).exists() {
            warn!("assignable device {device:?} doesn't exist; ignoring...");
            return false;
        }

        device_set.insert(device.sysfs_path.clone());
        true
    });
    Ok(devices)
}

fn split_x509_certificate_chain(mut cert_chain: &[u8]) -> Result<Vec<Certificate>> {
    let mut out = Vec::new();
    while !cert_chain.is_empty() {
        let cert = X509::from_der(cert_chain)?;
        let end = cert.to_der()?.len();
        out.push(Certificate { encodedCertificate: cert_chain[..end].to_vec() });
        cert_chain = &cert_chain[end..];
    }
    Ok(out)
}

#[derive(Debug, Default)]
struct GlobalVmInstance {
    /// The unique CID assigned to the VM for vsock communication.
    cid: Cid,
    /// UID of the client who requested this VM instance.
    requester_uid: uid_t,
    /// PID of the client who requested this VM instance.
    requester_debug_pid: pid_t,
    /// Name of the host console.
    host_console_name: Option<String>,
}

impl GlobalVmInstance {
    fn get_temp_dir(&self) -> PathBuf {
        let cid = self.cid;
        format!("{TEMPORARY_DIRECTORY}/{cid}").into()
    }
}

/// The mutable state of the VirtualizationServiceInternal. There should only be one instance
/// of this struct.
struct GlobalState {
    /// VM contexts currently allocated to running VMs. A CID is never recycled as long
    /// as there is a strong reference held by a GlobalVmContext.
    held_contexts: HashMap<Cid, Weak<Mutex<GlobalVmInstance>>>,

    /// Cached read-only FD of VM DTBO file. Also serves as a lock for creating the file.
    dtbo_file: Mutex<Option<File>>,

    /// State relating to secrets held by (optional) Secretkeeper instance on behalf of VMs.
    sk_state: Option<maintenance::State>,

    display_service: Option<binder::SpIBinder>,
}

impl GlobalState {
    fn new() -> Self {
        Self {
            held_contexts: HashMap::new(),
            dtbo_file: Mutex::new(None),
            sk_state: maintenance::State::new(),
            display_service: None,
        }
    }

    /// Get the next available CID, or an error if we have run out. The last CID used is stored in
    /// a system property so that restart of virtualizationservice doesn't reuse CID while the host
    /// Android is up.
    fn get_next_available_cid(&mut self) -> Result<Cid> {
        // Start trying to find a CID from the last used CID + 1. This ensures
        // that we do not eagerly recycle CIDs. It makes debugging easier but
        // also means that retrying to allocate a CID, eg. because it is
        // erroneously occupied by a process, will not recycle the same CID.
        let last_cid_prop =
            system_properties::read(SYSPROP_LAST_CID)?.and_then(|val| match val.parse::<Cid>() {
                Ok(num) => {
                    if is_valid_guest_cid(num) {
                        Some(num)
                    } else {
                        error!("Invalid value '{}' of property '{}'", num, SYSPROP_LAST_CID);
                        None
                    }
                }
                Err(_) => {
                    error!("Invalid value '{}' of property '{}'", val, SYSPROP_LAST_CID);
                    None
                }
            });

        let first_cid = if let Some(last_cid) = last_cid_prop {
            if last_cid == GUEST_CID_MAX {
                GUEST_CID_MIN
            } else {
                last_cid + 1
            }
        } else {
            GUEST_CID_MIN
        };

        let cid = self
            .find_available_cid(first_cid..=GUEST_CID_MAX)
            .or_else(|| self.find_available_cid(GUEST_CID_MIN..first_cid))
            .ok_or_else(|| anyhow!("Could not find an available CID."))?;

        system_properties::write(SYSPROP_LAST_CID, &format!("{}", cid))?;
        Ok(cid)
    }

    fn find_available_cid<I>(&self, mut range: I) -> Option<Cid>
    where
        I: Iterator<Item = Cid>,
    {
        range.find(|cid| !self.held_contexts.contains_key(cid))
    }

    fn allocate_vm_context(
        &mut self,
        requester_uid: uid_t,
        requester_debug_pid: pid_t,
    ) -> Result<Strong<dyn IGlobalVmContext>> {
        // Garbage collect unused VM contexts.
        self.held_contexts.retain(|_, instance| instance.strong_count() > 0);

        let cid = self.get_next_available_cid()?;
        let instance = Arc::new(Mutex::new(GlobalVmInstance {
            cid,
            requester_uid,
            requester_debug_pid,
            ..Default::default()
        }));
        create_temporary_directory(&instance.lock().unwrap().get_temp_dir(), Some(requester_uid))?;

        self.held_contexts.insert(cid, Arc::downgrade(&instance));
        let binder = GlobalVmContext { instance, ..Default::default() };
        Ok(BnGlobalVmContext::new_binder(binder, BinderFeatures::default()))
    }

    fn get_dtbo_file(&mut self) -> Result<File> {
        let mut file = self.dtbo_file.lock().unwrap();

        let fd = if let Some(ref_fd) = &*file {
            ref_fd.try_clone()?
        } else {
            let path = get_or_create_common_dir()?.join("vm.dtbo");
            if path.exists() {
                // All temporary files are deleted when the service is started.
                // If the file exists but the FD is not cached, the file is
                // likely corrupted.
                remove_file(&path).context("Failed to clone cached VM DTBO file descriptor")?;
            }

            // Open a write-only file descriptor for vfio_handler.
            let write_fd = File::create(&path).context("Failed to create VM DTBO file")?;
            VFIO_SERVICE.writeVmDtbo(&ParcelFileDescriptor::new(write_fd))?;

            // Open read-only. This FD will be cached and returned to clients.
            let read_fd = File::open(&path).context("Failed to open VM DTBO file")?;
            let read_fd_clone =
                read_fd.try_clone().context("Failed to clone VM DTBO file descriptor")?;
            *file = Some(read_fd);
            read_fd_clone
        };

        Ok(fd)
    }
}

fn create_temporary_directory(path: &PathBuf, requester_uid: Option<uid_t>) -> Result<()> {
    // Directory may exist if previous attempt to create it had failed.
    // Delete it before trying again.
    if path.as_path().exists() {
        remove_temporary_dir(path).unwrap_or_else(|e| {
            warn!("Could not delete temporary directory {:?}: {}", path, e);
        });
    }
    // Create directory.
    create_dir(path).with_context(|| format!("Could not create temporary directory {:?}", path))?;
    // If provided, change ownership to client's UID but system's GID, and permissions 0700.
    // If the chown() fails, this will leave behind an empty directory that will get removed
    // at the next attempt, or if virtualizationservice is restarted.
    if let Some(uid) = requester_uid {
        chown(path, Some(Uid::from_raw(uid)), None).with_context(|| {
            format!("Could not set ownership of temporary directory {:?}", path)
        })?;
    }
    Ok(())
}

/// Removes a directory owned by a different user by first changing its owner back
/// to VirtualizationService.
pub fn remove_temporary_dir(path: &PathBuf) -> Result<()> {
    ensure!(path.as_path().is_dir(), "Path {:?} is not a directory", path);
    chown(path, Some(Uid::current()), None)?;
    set_permissions(path, Permissions::from_mode(0o700))?;
    remove_dir_all(path)?;
    Ok(())
}

fn get_or_create_common_dir() -> Result<PathBuf> {
    let path = Path::new(TEMPORARY_DIRECTORY).join("common");
    if !path.exists() {
        create_temporary_directory(&path, None)?;
    }
    Ok(path)
}

/// Implementation of the AIDL `IGlobalVmContext` interface.
#[derive(Debug, Default)]
struct GlobalVmContext {
    /// Strong reference to the context's instance data structure.
    instance: Arc<Mutex<GlobalVmInstance>>,
    /// Keeps our service process running as long as this VM context exists.
    #[allow(dead_code)]
    lazy_service_guard: LazyServiceGuard,
}

impl Interface for GlobalVmContext {}

impl IGlobalVmContext for GlobalVmContext {
    fn getCid(&self) -> binder::Result<i32> {
        Ok(self.instance.lock().unwrap().cid as i32)
    }

    fn getTemporaryDirectory(&self) -> binder::Result<String> {
        Ok(self.instance.lock().unwrap().get_temp_dir().to_string_lossy().to_string())
    }

    fn setHostConsoleName(&self, pathname: &str) -> binder::Result<()> {
        self.instance.lock().unwrap().host_console_name = Some(pathname.to_string());
        Ok(())
    }
}

fn handle_stream_connection_tombstoned() -> Result<()> {
    // Should not listen for tombstones on a guest VM's port.
    assert!(!is_valid_guest_cid(VM_TOMBSTONES_SERVICE_PORT as Cid));
    let listener =
        VsockListener::bind_with_cid_port(VMADDR_CID_HOST, VM_TOMBSTONES_SERVICE_PORT as Cid)?;
    for incoming_stream in listener.incoming() {
        let mut incoming_stream = match incoming_stream {
            Err(e) => {
                warn!("invalid incoming connection: {e:?}");
                continue;
            }
            Ok(s) => s,
        };
        if let Ok(addr) = incoming_stream.peer_addr() {
            let cid = addr.cid();
            match cid {
                VMADDR_CID_LOCAL | VMADDR_CID_HOST | VMADDR_CID_HYPERVISOR => {
                    warn!("Rejecting non-guest tombstone vsock connection from cid={cid}");
                    continue;
                }
                _ => info!("Vsock Stream connected to cid={cid} for tombstones"),
            }
        }
        std::thread::spawn(move || {
            if let Err(e) = handle_tombstone(&mut incoming_stream) {
                error!("Failed to write tombstone- {:?}", e);
            }
        });
    }
    Ok(())
}

fn handle_tombstone(stream: &mut VsockStream) -> Result<()> {
    let tb_connection =
        TombstonedConnection::connect(std::process::id() as i32, DebuggerdDumpType::Tombstone)
            .context("Failed to connect to tombstoned")?;
    let mut text_output = tb_connection
        .text_output
        .as_ref()
        .ok_or_else(|| anyhow!("Could not get file to write the tombstones on"))?;
    let mut num_bytes_read = 0;
    loop {
        let mut chunk_recv = [0; CHUNK_RECV_MAX_LEN];
        let n = stream
            .read(&mut chunk_recv)
            .context("Failed to read tombstone data from Vsock stream")?;
        if n == 0 {
            break;
        }
        num_bytes_read += n;
        text_output.write_all(&chunk_recv[0..n]).context("Failed to write guests tombstones")?;
    }
    info!("Received {} bytes from guest & wrote to tombstone file", num_bytes_read);
    tb_connection.notify_completion()?;
    Ok(())
}

/// Returns true if the AVF remotely provisioned component service is declared in the
/// VINTF manifest.
pub(crate) fn is_remote_provisioning_hal_declared() -> binder::Result<bool> {
    Ok(binder::is_declared(REMOTELY_PROVISIONED_COMPONENT_SERVICE_NAME)?)
}

/// Checks whether the caller has a specific permission
fn check_permission(perm: &str) -> binder::Result<()> {
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

/// Check whether the caller of the current Binder method is allowed to call debug methods.
fn check_debug_access() -> binder::Result<()> {
    check_permission("android.permission.DEBUG_VIRTUAL_MACHINE")
}

/// Check whether the caller of the current Binder method is allowed to manage VMs
fn check_manage_access() -> binder::Result<()> {
    check_permission("android.permission.MANAGE_VIRTUAL_MACHINE")
}

/// Check whether the caller of the current Binder method is allowed to use custom VMs
fn check_use_custom_virtual_machine() -> binder::Result<()> {
    check_permission("android.permission.USE_CUSTOM_VIRTUAL_MACHINE")
}

/// Check whether the caller of the current Binder method is allowed to create socket and
/// establish connection between the VM and the Internet.
fn check_internet_permission() -> binder::Result<()> {
    check_permission("android.permission.INTERNET")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_RKP_CERT_CHAIN_PATH: &str = "testdata/rkp_cert_chain.der";

    #[test]
    fn splitting_x509_certificate_chain_succeeds() -> Result<()> {
        let bytes = fs::read(TEST_RKP_CERT_CHAIN_PATH)?;
        let cert_chain = split_x509_certificate_chain(&bytes)?;

        assert_eq!(4, cert_chain.len());
        for cert in cert_chain {
            let x509_cert = X509::from_der(&cert.encodedCertificate)?;
            assert_eq!(x509_cert.to_der()?.len(), cert.encodedCertificate.len());
        }
        Ok(())
    }
}
