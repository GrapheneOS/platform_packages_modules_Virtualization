// Copyright 2023, The Android Open Source Project
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

//! Handles the RKP (Remote Key Provisioning) VM and host communication.
//! The RKP VM will be recognized and attested by the RKP server periodically and
//! serves as a trusted platform to attest a client VM.

use android_hardware_security_rkp::aidl::android::hardware::security::keymint::MacedPublicKey::MacedPublicKey;
use android_trusty_vm_attestation::aidl::android::trusty::vm_attestation::IVmAttestation::{
    IVmAttestation, MacedPublicKey::MacedPublicKey as VmAttestMacedPublicKey,
};
use anyhow::{anyhow, bail, Context, Result};
use binder::{AccessorProvider, Strong};
use service_vm_comm::{
    ClientVmAttestationParams, EcdsaP256KeyPair, GenerateCertificateRequestParams, Request,
    Response,
};
use service_vm_manager::process_request;

const VM_ATTESTATION_ACCESSOR_SERVICE_NAME: &str = "android.os.IAccessor/IVmAttestation/default";
const VM_ATTESTATION_RPC_SERVICE_NAME: &str =
    "android.trusty.vm_attestation.IVmAttestation/default";

pub(crate) fn get_vm_attestation_accessor_provider() -> Result<AccessorProvider> {
    AccessorProvider::new(&[VM_ATTESTATION_RPC_SERVICE_NAME.to_owned()], |s| {
        binder::wait_for_service(VM_ATTESTATION_ACCESSOR_SERVICE_NAME)
            .and_then(|service| binder::Accessor::from_binder(s, service))
    })
    .ok_or(anyhow!("Failed to create access provider"))
}

fn get_vm_attestation_service() -> Result<Strong<dyn IVmAttestation>> {
    Ok(binder::get_interface(VM_ATTESTATION_RPC_SERVICE_NAME)?)
}

pub(crate) fn request_attestation(
    csr: Vec<u8>,
    remotely_provisioned_key_blob: Vec<u8>,
    remotely_provisioned_cert: Vec<u8>,
) -> Result<Vec<u8>> {
    if cfg!(use_vm_attestation_service_accessor) {
        let vm_attestation_service = get_vm_attestation_service()?;
        return Ok(vm_attestation_service.requestAttestation(
            &csr,
            &remotely_provisioned_key_blob,
            &remotely_provisioned_cert,
        )?);
    }

    let params =
        ClientVmAttestationParams { csr, remotely_provisioned_key_blob, remotely_provisioned_cert };
    let request = Request::RequestClientVmAttestation(params);
    match process_request(request).context("Failed to process request")? {
        Response::RequestClientVmAttestation(cert) => Ok(cert),
        other => bail!("Incorrect response type {other:?}"),
    }
}

pub(crate) fn generate_ecdsa_p256_key_pair() -> Result<Response> {
    if cfg!(use_vm_attestation_service_accessor) {
        let vm_attestation_service = get_vm_attestation_service()?;
        let res = vm_attestation_service.generateEcdsaP256KeyPair()?;
        return Ok(Response::GenerateEcdsaP256KeyPair(EcdsaP256KeyPair {
            maced_public_key: res.macedPublicKey,
            key_blob: res.keyBlob,
        }));
    }

    let request = Request::GenerateEcdsaP256KeyPair;
    process_request(request).context("Failed to process request")
}

pub(crate) fn generate_certificate_request(
    keys_to_sign: &[MacedPublicKey],
    challenge: &[u8],
) -> Result<Response> {
    if cfg!(use_vm_attestation_service_accessor) {
        let vm_attestation_service = get_vm_attestation_service()?;
        let kts = keys_to_sign
            .iter()
            .map(|k| VmAttestMacedPublicKey { macedPublicKey: k.macedKey.clone() })
            .collect::<Vec<VmAttestMacedPublicKey>>();
        return Ok(Response::GenerateCertificateRequest(
            vm_attestation_service.generateCertificateRequest(challenge, &kts)?,
        ));
    }

    let params = GenerateCertificateRequestParams {
        keys_to_sign: keys_to_sign.iter().map(|v| v.macedKey.to_vec()).collect(),
        challenge: challenge.to_vec(),
    };
    let request = Request::GenerateCertificateRequest(params);

    process_request(request).context("Failed to process request")
}
