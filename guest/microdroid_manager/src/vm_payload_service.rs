// Copyright 2022, The Android Open Source Project
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

//! Implementation of the AIDL interface `IVmPayloadService`.

use android_system_virtualization_payload::aidl::android::system::virtualization::payload::IVmPayloadService::{
    IVmPayloadService, AttestationResult::AttestationResult, ENCRYPTEDSTORE_MOUNTPOINT,
    MICRODROID_SOCKET_PATH, STATUS_FAILED_TO_PREPARE_CSR_AND_KEY, VM_APK_CONTENTS_PATH
};
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use anyhow::{anyhow, Context, Result};
use avflog::LogResult;
use binder::{ExceptionCode, Interface, IntoBinderResult, ParcelFileDescriptor, Status, Strong};
use client_vm_csr::{generate_attestation_key_and_csr, ClientVmAttestationData};
use crate::encrypted_assets::{mount_encrypted_assets, MountError};
use crate::tenant::TenantManager;
use crate::vm_secret::VmSecret;
use libc::uid_t;
use log::{error, info};
use microdroid_uids::MICRODROID_PAYLOAD_UID;
use std::path::Path;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// State shared across all VmPayloadService sessions.
pub struct VmPayloadServiceShared {
    pub virtual_machine_service: Strong<dyn IVirtualMachineService>,
    pub allow_restricted_apis: bool,
    pub secret: Arc<VmSecret>,
    pub is_new_instance: bool,
    pub total_tasks: usize,
    pub tasks_ready: AtomicUsize,
    pub tenant_manager: Arc<TenantManager>,
}

/// Implementation of `IVmPayloadService`. A new instance is created for each client.
pub(crate) struct VmPayloadService {
    shared: Arc<VmPayloadServiceShared>,
    client_uid: Option<uid_t>,
}

impl IVmPayloadService for VmPayloadService {
    fn notifyPayloadReady(&self) -> binder::Result<()> {
        info!("notifyPayloadReady called by client with UID: {:?}", self.client_uid);

        let tasks_ready = self.shared.tasks_ready.fetch_add(1, Ordering::SeqCst) + 1;
        if tasks_ready == self.shared.total_tasks {
            self.shared
                .virtual_machine_service
                .notifyPayloadReady()
                .inspect(|_| info!("Notified host payload ready successfully"))
                .inspect_err(|e| error!("Failed to notify host about payload ready: {e:?}"))
        } else {
            info!(
                "Received {} of {} payload ready notifications.",
                tasks_ready, self.shared.total_tasks
            );
            Ok(())
        }
    }

    fn getVmInstanceSecret(&self, identifier: &[u8], size: i32) -> binder::Result<Vec<u8>> {
        if !(0..=32).contains(&size) {
            return Err(anyhow!("size {size} not in range (0..=32)"))
                .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
        }
        let mut instance_secret = vec![0; size.try_into().unwrap()];
        self.shared
            .secret
            .derive_payload_sealing_key(identifier, &mut instance_secret)
            .context("Failed to derive VM instance secret")
            .with_log()
            .or_service_specific_exception(-1)?;
        Ok(instance_secret)
    }

    fn getDiceAttestationChain(&self) -> binder::Result<Vec<u8>> {
        self.check_restricted_apis_allowed()?;
        if let Some(bcc) = self.shared.secret.dice_artifacts().bcc() {
            Ok(bcc.to_vec())
        } else {
            Err(anyhow!("bcc is none")).or_binder_exception(ExceptionCode::ILLEGAL_STATE)
        }
    }

    fn getDiceAttestationCdi(&self) -> binder::Result<Vec<u8>> {
        self.check_restricted_apis_allowed()?;
        Ok(self.shared.secret.dice_artifacts().cdi_attest().to_vec())
    }

    fn requestAttestation(
        &self,
        challenge: &[u8],
        test_mode: bool,
    ) -> binder::Result<AttestationResult> {
        let ClientVmAttestationData { private_key, csr } =
            generate_attestation_key_and_csr(challenge, self.shared.secret.dice_artifacts())
                .map_err(|e| {
                    Status::new_service_specific_error_str(
                        STATUS_FAILED_TO_PREPARE_CSR_AND_KEY,
                        Some(format!("Failed to prepare the CSR and key pair: {e:?}")),
                    )
                })
                .with_log()?;
        let csr = csr
            .into_cbor_vec()
            .map_err(|e| {
                Status::new_service_specific_error_str(
                    STATUS_FAILED_TO_PREPARE_CSR_AND_KEY,
                    Some(format!("Failed to serialize CSR into CBOR: {e:?}")),
                )
            })
            .with_log()?;
        let cert_chain = self.shared.virtual_machine_service.requestAttestation(&csr, test_mode)?;
        Ok(AttestationResult {
            privateKey: private_key.as_slice().to_vec(),
            certificateChain: cert_chain,
        })
    }

    fn readPayloadRpData(&self) -> binder::Result<Option<[u8; 32]>> {
        let data = self
            .shared
            .secret
            .read_payload_data_rp()
            .context("Failed to read payload's rollback protected data")
            .with_log()
            .or_service_specific_exception(-1)?;
        Ok(data)
    }

    fn writePayloadRpData(&self, data: &[u8; 32]) -> binder::Result<()> {
        self.shared
            .secret
            .write_payload_data_rp(data)
            .context("Failed to write payload's rollback protected data")
            .with_log()
            .or_service_specific_exception(-1)?;
        Ok(())
    }

    fn isNewInstance(&self) -> binder::Result<bool> {
        Ok(self.shared.is_new_instance)
    }

    fn mountEncryptedAssets(
        &self,
        image_path: &str,
        fs_type: &str,
        cipher: &str,
        key: &[u8],
        sector_size: i32,
    ) -> binder::Result<String> {
        if self.shared.total_tasks > 1 {
            // TODO(b/425553329): Add a test for this scenario.
            return Err(anyhow!(
                "Mounting encrypted assets is not supported in multi-tenant payloads"
            ))
            .or_service_specific_exception(-1);
        }
        mount_encrypted_assets(image_path, fs_type, cipher, key, sector_size)
            .context("Failed to mount encrypted assets")
            .with_log()
            .map_err(|e| match e.downcast_ref::<MountError>() {
                Some(MountError::BadImage)
                | Some(MountError::BadFsType)
                | Some(MountError::BadCipher)
                | Some(MountError::BadKeySize)
                | Some(MountError::BadSectorSize) => Status::new_exception_str(
                    ExceptionCode::ILLEGAL_ARGUMENT,
                    Some(format!("{e:?}")),
                ),
                Some(MountError::Other) | None => {
                    Status::new_service_specific_error_str(-1, Some(format!("{e:?}")))
                }
            })
    }

    fn getEncryptedStoragePath(&self, uid: i64) -> binder::Result<String> {
        if Path::new(ENCRYPTEDSTORE_MOUNTPOINT).exists() {
            if uid == MICRODROID_PAYLOAD_UID as i64 {
                return Ok(ENCRYPTEDSTORE_MOUNTPOINT.to_string());
            }
            let package_name = self
                .shared
                .tenant_manager
                .get_tenant_package_name(uid)
                .context("Failed to get tenant package name for encrypted storage path")
                .with_log()
                .or_service_specific_exception(-1)?;
            return Ok(format!("{}/{}", ENCRYPTEDSTORE_MOUNTPOINT, package_name));
        }
        Ok("".to_string())
    }

    fn createUnixDomainSocket(&self, name: &str) -> binder::Result<ParcelFileDescriptor> {
        self.create_unix_domain_socket_internal(name).with_log().or_service_specific_exception(-1)
    }

    fn getApkContentsPath(&self, uid: i64) -> binder::Result<String> {
        if uid == MICRODROID_PAYLOAD_UID as i64 {
            return Ok(VM_APK_CONTENTS_PATH.to_string());
        }
        let package_name = self
            .shared
            .tenant_manager
            .get_tenant_package_name(uid)
            .context("Failed to get tenant package name for APK contents path")
            .with_log()
            .or_service_specific_exception(-1)?;
        Ok(format!("{}/{}", VM_APK_CONTENTS_PATH, package_name))
    }
}

impl Interface for VmPayloadService {}

impl VmPayloadService {
    /// Creates a new `VmPayloadService` instance.
    pub(crate) fn new(
        shared: Arc<VmPayloadServiceShared>,
        client_uid: Option<uid_t>,
    ) -> VmPayloadService {
        Self { shared, client_uid }
    }

    fn check_restricted_apis_allowed(&self) -> binder::Result<()> {
        if self.shared.allow_restricted_apis {
            Ok(())
        } else {
            Err(anyhow!("Use of restricted APIs is not allowed"))
                .with_log()
                .or_binder_exception(ExceptionCode::SECURITY)
        }
    }

    fn create_unix_domain_socket_internal(&self, name: &str) -> Result<ParcelFileDescriptor> {
        let socket_path = format!("{}/{}", MICRODROID_SOCKET_PATH, name);

        // TODO(b/460097396) : Integrate with access control
        let listener = UnixListener::bind(&socket_path).with_context(|| {
            format!("Failed to bind socket at {socket_path}. It may already be in use.")
        })?;

        let permissions = std::fs::Permissions::from_mode(0o666);
        fs::set_permissions(&socket_path, permissions)
            .context("Failed to set socket permissions")?;
        // The listener's file descriptor is returned, effectively "leaking" it to the caller.
        Ok(ParcelFileDescriptor::new(listener))
    }
}
