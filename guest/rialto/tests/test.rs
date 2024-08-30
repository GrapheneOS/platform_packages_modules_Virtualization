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

//! Integration test for Rialto.

use android_system_virtualizationservice::{
    aidl::android::system::virtualizationservice::{
        VirtualMachineConfig::VirtualMachineConfig,
        VirtualMachineRawConfig::VirtualMachineRawConfig,
    },
    binder::{ParcelFileDescriptor, ProcessState},
};
use anyhow::{bail, Context, Result};
use bssl_avf::{rand_bytes, sha256, EcKey, PKey};
use client_vm_csr::generate_attestation_key_and_csr;
use coset::{CborSerializable, CoseMac0, CoseSign};
use hwtrust::{rkp, session::Session};
use log::{info, warn};
use service_vm_comm::{
    ClientVmAttestationParams, Csr, CsrPayload, EcdsaP256KeyPair, GenerateCertificateRequestParams,
    Request, RequestProcessingError, Response, VmType,
};
use service_vm_fake_chain::client_vm::{
    fake_client_vm_dice_artifacts, fake_sub_components, SubComponent,
};
use service_vm_manager::{ServiceVm, VM_MEMORY_MB};
use std::fs;
use std::fs::File;
use std::panic;
use std::path::PathBuf;
use std::str::FromStr;
use vmclient::VmInstance;
use x509_cert::{
    certificate::{Certificate, Version},
    der::{self, asn1, Decode, Encode},
    name::Name,
    spki::{AlgorithmIdentifier, ObjectIdentifier, SubjectPublicKeyInfo},
};

const UNSIGNED_RIALTO_PATH: &str = "/data/local/tmp/rialto_test/arm64/rialto_unsigned.bin";
const INSTANCE_IMG_PATH: &str = "/data/local/tmp/rialto_test/arm64/instance.img";
const TEST_CERT_CHAIN_PATH: &str = "testdata/rkp_cert_chain.der";

#[cfg(dice_changes)]
#[test]
fn process_requests_in_protected_vm() -> Result<()> {
    if hypervisor_props::is_protected_vm_supported()? {
        // The test is skipped if the feature flag |dice_changes| is not enabled, because when
        // the flag is off, the DICE chain is truncated in the pvmfw, and the service VM cannot
        // verify the chain due to the missing entries in the chain.
        check_processing_requests(VmType::ProtectedVm, None)
    } else {
        warn!("pVMs are not supported on device, skipping test");
        Ok(())
    }
}

#[test]
fn process_requests_in_non_protected_vm() -> Result<()> {
    const MEMORY_MB: i32 = 300;
    check_processing_requests(VmType::NonProtectedVm, Some(MEMORY_MB))?;
    check_processing_requests(VmType::NonProtectedVm, None)
}

fn check_processing_requests(vm_type: VmType, vm_memory_mb: Option<i32>) -> Result<()> {
    let mut vm = start_service_vm(vm_type, vm_memory_mb)?;

    check_processing_reverse_request(&mut vm)?;
    let key_pair = check_processing_generating_key_pair_request(&mut vm)?;
    check_processing_generating_certificate_request(&mut vm, &key_pair.maced_public_key)?;
    check_attestation_request(&mut vm, &key_pair, vm_type)?;
    Ok(())
}

fn check_processing_reverse_request(vm: &mut ServiceVm) -> Result<()> {
    let message = "abc".repeat(500);
    let request = Request::Reverse(message.as_bytes().to_vec());

    let response = vm.process_request(request)?;
    info!("Received response: {response:?}.");

    let expected_response: Vec<u8> = message.as_bytes().iter().rev().cloned().collect();
    assert_eq!(Response::Reverse(expected_response), response);
    Ok(())
}

fn check_processing_generating_key_pair_request(vm: &mut ServiceVm) -> Result<EcdsaP256KeyPair> {
    let request = Request::GenerateEcdsaP256KeyPair;

    let response = vm.process_request(request)?;
    info!("Received response: {response:?}.");

    match response {
        Response::GenerateEcdsaP256KeyPair(key_pair) => {
            assert_array_has_nonzero(&key_pair.maced_public_key);
            assert_array_has_nonzero(&key_pair.key_blob);
            Ok(key_pair)
        }
        _ => bail!("Incorrect response type: {response:?}"),
    }
}

fn assert_array_has_nonzero(v: &[u8]) {
    assert!(v.iter().any(|&x| x != 0))
}

fn check_processing_generating_certificate_request(
    vm: &mut ServiceVm,
    maced_public_key: &[u8],
) -> Result<()> {
    let params = GenerateCertificateRequestParams {
        keys_to_sign: vec![maced_public_key.to_vec()],
        challenge: vec![],
    };
    let request = Request::GenerateCertificateRequest(params);

    let response = vm.process_request(request)?;
    info!("Received response: {response:?}.");

    match response {
        Response::GenerateCertificateRequest(csr) => check_csr(csr),
        _ => bail!("Incorrect response type: {response:?}"),
    }
}

fn check_attestation_request(
    vm: &mut ServiceVm,
    remotely_provisioned_key_pair: &EcdsaP256KeyPair,
    vm_type: VmType,
) -> Result<()> {
    /// The following data was generated randomly with urandom.
    const CHALLENGE: [u8; 16] = [
        0x7d, 0x86, 0x58, 0x79, 0x3a, 0x09, 0xdf, 0x1c, 0xa5, 0x80, 0x80, 0x15, 0x2b, 0x13, 0x17,
        0x5c,
    ];
    let dice_artifacts = fake_client_vm_dice_artifacts()?;
    let attestation_data = generate_attestation_key_and_csr(&CHALLENGE, &dice_artifacts)?;
    let cert_chain = fs::read(TEST_CERT_CHAIN_PATH)?;
    // The certificate chain contains several certificates, but we only need the first one.
    // Parsing the data with trailing data always fails with a `TrailingData` error.
    let cert_len: usize = match Certificate::from_der(&cert_chain).unwrap_err().kind() {
        der::ErrorKind::TrailingData { decoded, .. } => decoded.try_into().unwrap(),
        e => bail!("Unexpected error: {e}"),
    };

    // Builds the mock parameters for the client VM attestation.
    // The `csr` and `remotely_provisioned_key_blob` parameters are extracted from the same
    // libraries as in production.
    // The `remotely_provisioned_cert` parameter is an RKP certificate extracted from a test
    // certificate chain retrieved from RKPD.
    let params = ClientVmAttestationParams {
        csr: attestation_data.csr.clone().into_cbor_vec()?,
        remotely_provisioned_key_blob: remotely_provisioned_key_pair.key_blob.to_vec(),
        remotely_provisioned_cert: cert_chain[..cert_len].to_vec(),
    };
    let request = Request::RequestClientVmAttestation(params);

    let response = vm.process_request(request)?;
    info!("Received response: {response:?}.");

    match response {
        Response::RequestClientVmAttestation(certificate) => {
            // The end-to-end test for non-protected VM attestation works because both the service
            // VM and the client VM use the same fake DICE chain.
            assert_eq!(vm_type, VmType::NonProtectedVm);
            check_certificate_for_client_vm(
                &certificate,
                &remotely_provisioned_key_pair.maced_public_key,
                &attestation_data.csr,
                &Certificate::from_der(&cert_chain[..cert_len]).unwrap(),
            )?;
            Ok(())
        }
        Response::Err(RequestProcessingError::InvalidDiceChain) => {
            // The end-to-end test for protected VM attestation doesn't work because the service VM
            // compares the fake DICE chain in the CSR with the real DICE chain.
            // We cannot generate a valid DICE chain with the same payloads up to pvmfw.
            assert_eq!(vm_type, VmType::ProtectedVm);
            Ok(())
        }
        _ => bail!("Incorrect response type: {response:?}"),
    }
}

fn check_vm_components(vm_components: &asn1::SequenceOf<asn1::Any, 4>) -> Result<()> {
    let expected_components = fake_sub_components();
    assert_eq!(expected_components.len(), vm_components.len());
    for (i, expected_component) in expected_components.iter().enumerate() {
        check_vm_component(vm_components.get(i).unwrap(), expected_component)?;
    }
    Ok(())
}

fn check_vm_component(vm_component: &asn1::Any, expected_component: &SubComponent) -> Result<()> {
    let vm_component = vm_component.decode_as::<asn1::SequenceOf<asn1::Any, 4>>().unwrap();
    assert_eq!(4, vm_component.len());
    let name = vm_component.get(0).unwrap().decode_as::<asn1::Utf8StringRef>().unwrap();
    assert_eq!(expected_component.name, name.as_ref());
    let version = vm_component.get(1).unwrap().decode_as::<u64>().unwrap();
    assert_eq!(expected_component.version, version);
    let code_hash = vm_component.get(2).unwrap().decode_as::<asn1::OctetString>().unwrap();
    assert_eq!(expected_component.code_hash, code_hash.as_bytes());
    let authority_hash = vm_component.get(3).unwrap().decode_as::<asn1::OctetString>().unwrap();
    assert_eq!(expected_component.authority_hash, authority_hash.as_bytes());
    Ok(())
}

fn check_certificate_for_client_vm(
    certificate: &[u8],
    maced_public_key: &[u8],
    csr: &Csr,
    parent_certificate: &Certificate,
) -> Result<()> {
    let cose_mac = CoseMac0::from_slice(maced_public_key)?;
    let authority_public_key =
        EcKey::from_cose_public_key_slice(&cose_mac.payload.unwrap()).unwrap();
    let cert = Certificate::from_der(certificate).unwrap();

    // Checks the certificate signature against the authority public key.
    const ECDSA_WITH_SHA_256: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
    let expected_algorithm = AlgorithmIdentifier { oid: ECDSA_WITH_SHA_256, parameters: None };
    assert_eq!(expected_algorithm, cert.signature_algorithm);
    let tbs_cert = cert.tbs_certificate;
    let digest = sha256(&tbs_cert.to_der().unwrap()).unwrap();
    authority_public_key
        .ecdsa_verify_der(cert.signature.raw_bytes(), &digest)
        .expect("Failed to verify the certificate signature with the authority public key");

    // Checks that the certificate's subject public key is equal to the key in the CSR.
    let cose_sign = CoseSign::from_slice(&csr.signed_csr_payload)?;
    let csr_payload =
        cose_sign.payload.as_ref().and_then(|v| CsrPayload::from_cbor_slice(v).ok()).unwrap();
    let subject_public_key = EcKey::from_cose_public_key_slice(&csr_payload.public_key).unwrap();
    let expected_spki_data =
        PKey::try_from(subject_public_key).unwrap().subject_public_key_info().unwrap();
    let expected_spki = SubjectPublicKeyInfo::from_der(&expected_spki_data).unwrap();
    assert_eq!(expected_spki, tbs_cert.subject_public_key_info);

    // Checks the certificate extension.
    const ATTESTATION_EXTENSION_OID: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.3.6.1.4.1.11129.2.1.29.1");
    let extensions = tbs_cert.extensions.unwrap();
    assert_eq!(1, extensions.len());
    let extension = &extensions[0];
    assert_eq!(ATTESTATION_EXTENSION_OID, extension.extn_id);
    assert!(!extension.critical);
    let attestation_ext =
        asn1::SequenceOf::<asn1::Any, 3>::from_der(extension.extn_value.as_bytes()).unwrap();
    assert_eq!(3, attestation_ext.len());
    let challenge = attestation_ext.get(0).unwrap().decode_as::<asn1::OctetString>().unwrap();
    assert_eq!(csr_payload.challenge, challenge.as_bytes());
    let is_vm_secure = attestation_ext.get(1).unwrap().decode_as::<bool>().unwrap();
    assert!(
        !is_vm_secure,
        "The VM shouldn't be secure as the last payload added in the test is in Debug mode"
    );
    let vm_components =
        attestation_ext.get(2).unwrap().decode_as::<asn1::SequenceOf<asn1::Any, 4>>().unwrap();
    check_vm_components(&vm_components)?;

    // Checks other fields on the certificate
    assert_eq!(Version::V3, tbs_cert.version);
    assert_eq!(parent_certificate.tbs_certificate.validity, tbs_cert.validity);
    assert_eq!(
        Name::from_str("CN=Android Protected Virtual Machine Key").unwrap(),
        tbs_cert.subject
    );
    assert_eq!(parent_certificate.tbs_certificate.subject, tbs_cert.issuer);

    Ok(())
}

fn check_csr(csr: Vec<u8>) -> Result<()> {
    let mut session = Session::default();
    session.set_allow_any_mode(true);
    let _csr = rkp::Csr::from_cbor(&session, &csr[..]).context("Failed to parse CSR")?;
    Ok(())
}

fn start_service_vm(vm_type: VmType, vm_memory_mb: Option<i32>) -> Result<ServiceVm> {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("rialto")
            .with_max_level(log::LevelFilter::Debug),
    );
    // Redirect panic messages to logcat.
    panic::set_hook(Box::new(|panic_info| {
        log::error!("{}", panic_info);
    }));
    // We need to start the thread pool for Binder to work properly, especially link_to_death.
    ProcessState::start_thread_pool();
    ServiceVm::start_vm(vm_instance(vm_type, vm_memory_mb)?, vm_type)
}

fn vm_instance(vm_type: VmType, vm_memory_mb: Option<i32>) -> Result<VmInstance> {
    match vm_type {
        VmType::ProtectedVm => {
            assert!(vm_memory_mb.is_none());
            service_vm_manager::protected_vm_instance(PathBuf::from(INSTANCE_IMG_PATH))
        }
        VmType::NonProtectedVm => nonprotected_vm_instance(vm_memory_mb.unwrap_or(VM_MEMORY_MB)),
    }
}

fn nonprotected_vm_instance(memory_mib: i32) -> Result<VmInstance> {
    let rialto = File::open(UNSIGNED_RIALTO_PATH).context("Failed to open Rialto kernel binary")?;
    // Do not use `#allocateInstanceId` to generate the instance ID because the method
    // also adds an instance ID to the database it manages.
    // This is not necessary for this test.
    let mut instance_id = [0u8; 64];
    rand_bytes(&mut instance_id).unwrap();
    let config = VirtualMachineConfig::RawConfig(VirtualMachineRawConfig {
        name: format!("Non protected rialto ({memory_mib}MiB)"),
        kernel: Some(ParcelFileDescriptor::new(rialto)),
        protectedVm: false,
        memoryMib: memory_mib,
        platformVersion: "~1.0".to_string(),
        instanceId: instance_id,
        ..Default::default()
    });
    let console = Some(service_vm_manager::android_log_fd()?);
    let log = Some(service_vm_manager::android_log_fd()?);
    let virtmgr = vmclient::VirtualizationService::new().context("Failed to spawn VirtMgr")?;
    let service = virtmgr.connect().context("Failed to connect to VirtMgr")?;
    info!("Connected to VirtMgr for service VM");
    VmInstance::create(service.as_ref(), &config, console, /* consoleIn */ None, log, None)
        .context("Failed to create VM")
}
