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
    IVmPayloadService, AttestationResult::AttestationResult,
    STATUS_FAILED_TO_PREPARE_CSR_AND_KEY,
};
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use anyhow::{anyhow, Context};
use avflog::LogResult;
use binder::{ExceptionCode, Interface, IntoBinderResult, Status, Strong};
use client_vm_csr::{generate_attestation_key_and_csr, ClientVmAttestationData};
use crate::encrypted_assets::{mount_encrypted_assets, MountError};
use crate::vm_secret::VmSecret;
use log::{error, info};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// Implementation of `IVmPayloadService`.
pub(crate) struct VmPayloadService {
    allow_restricted_apis: bool,
    virtual_machine_service: Strong<dyn IVirtualMachineService>,
    secret: Arc<VmSecret>,
    is_new_instance: bool,
    total_tasks: usize,
    tasks_ready: AtomicUsize,
}

impl IVmPayloadService for VmPayloadService {
    fn notifyPayloadReady(&self) -> binder::Result<()> {
        let tasks_ready = self.tasks_ready.fetch_add(1, Ordering::SeqCst) + 1;
        if tasks_ready == self.total_tasks {
            self.virtual_machine_service
                .notifyPayloadReady()
                .inspect(|_| info!("Notified host payload ready successfully"))
                .inspect_err(|e| error!("Failed to notify host about payload ready: {e:?}"))
        } else {
            info!("Received {} of {} payload ready notifications.", tasks_ready, self.total_tasks);
            Ok(())
        }
    }

    fn getVmInstanceSecret(&self, identifier: &[u8], size: i32) -> binder::Result<Vec<u8>> {
        if !(0..=32).contains(&size) {
            return Err(anyhow!("size {size} not in range (0..=32)"))
                .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
        }
        let mut instance_secret = vec![0; size.try_into().unwrap()];
        self.secret
            .derive_payload_sealing_key(identifier, &mut instance_secret)
            .context("Failed to derive VM instance secret")
            .with_log()
            .or_service_specific_exception(-1)?;
        Ok(instance_secret)
    }

    fn getDiceAttestationChain(&self) -> binder::Result<Vec<u8>> {
        self.check_restricted_apis_allowed()?;
        if let Some(bcc) = self.secret.dice_artifacts().bcc() {
            Ok(bcc.to_vec())
        } else {
            Err(anyhow!("bcc is none")).or_binder_exception(ExceptionCode::ILLEGAL_STATE)
        }
    }

    fn getDiceAttestationCdi(&self) -> binder::Result<Vec<u8>> {
        self.check_restricted_apis_allowed()?;
        Ok(self.secret.dice_artifacts().cdi_attest().to_vec())
    }

    fn requestAttestation(
        &self,
        challenge: &[u8],
        test_mode: bool,
    ) -> binder::Result<AttestationResult> {
        let ClientVmAttestationData { private_key, csr } =
            generate_attestation_key_and_csr(challenge, self.secret.dice_artifacts())
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
        let cert_chain = self.virtual_machine_service.requestAttestation(&csr, test_mode)?;
        Ok(AttestationResult {
            privateKey: private_key.as_slice().to_vec(),
            certificateChain: cert_chain,
        })
    }

    fn readPayloadRpData(&self) -> binder::Result<Option<[u8; 32]>> {
        let data = self
            .secret
            .read_payload_data_rp()
            .context("Failed to read payload's rollback protected data")
            .with_log()
            .or_service_specific_exception(-1)?;
        Ok(data)
    }

    fn writePayloadRpData(&self, data: &[u8; 32]) -> binder::Result<()> {
        self.secret
            .write_payload_data_rp(data)
            .context("Failed to write payload's rollback protected data")
            .with_log()
            .or_service_specific_exception(-1)?;
        Ok(())
    }

    fn isNewInstance(&self) -> binder::Result<bool> {
        Ok(self.is_new_instance)
    }

    fn mountEncryptedAssets(
        &self,
        image_path: &str,
        fs_type: &str,
        cipher: &str,
        key: &[u8],
        sector_size: i32,
    ) -> binder::Result<String> {
        if self.total_tasks > 1 {
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
}

impl Interface for VmPayloadService {}

impl VmPayloadService {
    /// Creates a new `VmPayloadService` instance from the `IVirtualMachineService` reference.
    pub(crate) fn new(
        allow_restricted_apis: bool,
        vm_service: Strong<dyn IVirtualMachineService>,
        secret: Arc<VmSecret>,
        is_new_instance: bool,
        total_tasks: usize,
    ) -> VmPayloadService {
        Self {
            allow_restricted_apis,
            virtual_machine_service: vm_service,
            secret,
            is_new_instance,
            total_tasks,
            tasks_ready: AtomicUsize::new(0),
        }
    }

    fn check_restricted_apis_allowed(&self) -> binder::Result<()> {
        if self.allow_restricted_apis {
            Ok(())
        } else {
            Err(anyhow!("Use of restricted APIs is not allowed"))
                .with_log()
                .or_binder_exception(ExceptionCode::SECURITY)
        }
    }
}
