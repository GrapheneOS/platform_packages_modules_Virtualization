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

//! This module contains functions related to the attestation of the
//! client VM.

use crate::cert;
use crate::dice::{ClientVmDiceChain, DiceChainEntryPayload};
use crate::keyblob::decrypt_private_key;
use crate::ops::AttestationOps;
use alloc::vec::Vec;
use bssl_crypto::{digest::Sha512, ec::P256, ecdsa};
use cbor_util::{get_label_value, parse_value_array};
use ciborium::value::Value;
use core::str::FromStr;
use coset::{
    iana::{self, EnumI64},
    Algorithm, CborSerializable, CoseKey, CoseSign, Label,
};
use der::{Decode, Encode};
use log::{error, info};
use service_vm_comm::{ClientVmAttestationParams, Csr, CsrPayload, RequestProcessingError, Result};
use x509_cert::{certificate::Certificate, name::Name};

const DICE_CDI_LEAF_SIGNATURE_INDEX: usize = 0;
const ATTESTATION_KEY_SIGNATURE_INDEX: usize = 1;

/// Requests an attestation certificate for a Client VM.
pub fn request_attestation(
    params: ClientVmAttestationParams,
    ops: &impl AttestationOps,
) -> Result<Vec<u8>> {
    let csr = Csr::from_cbor_slice(&params.csr)?;
    let cose_sign = CoseSign::from_slice(&csr.signed_csr_payload)?;
    let csr_payload = cose_sign.payload.as_ref().ok_or_else(|| {
        error!("No CsrPayload found in the CSR");
        RequestProcessingError::InternalError
    })?;
    let csr_payload = CsrPayload::from_cbor_slice(csr_payload)?;

    let client_vm_dice_chain = validate_client_vm_dice_chain(&csr.dice_cert_chain, ops)?;

    // AAD is empty as defined in libs/libservice_vm_comm/client_vm_csr.cddl.
    let aad = &[];

    // Verifies the first signature with the leaf private key in the DICE chain.
    cose_sign.verify_signature(DICE_CDI_LEAF_SIGNATURE_INDEX, aad, |signature, message| {
        client_vm_dice_chain.microdroid_payload().subject_public_key.verify(signature, message)
    })?;

    // Verifies the second signature with the public key in the CSR payload.
    let ec_public_key = p256_cose_public_key_from_slice(&csr_payload.public_key)?;
    cose_sign
        .verify_signature(ATTESTATION_KEY_SIGNATURE_INDEX, aad, |signature, message| {
            ec_public_key.verify_p1363(message, signature)
        })
        .map_err(|_| RequestProcessingError::InvalidDiceChain)?;
    let subject_public_key_info = ec_public_key.to_der_subject_public_key_info();

    // Builds the TBSCertificate.
    // The serial number can be up to 20 bytes according to RFC5280 s4.1.2.2.
    // In this case, a serial number with a length of 16 bytes is used to ensure that each
    // certificate signed by RKP VM has a unique serial number.
    // Attention: Do not use 20 bytes here as when the MSB is 1, a leading 0 byte can be
    // added during the encoding to make the serial number length exceed 20 bytes.
    let serial_number: [u8; 16] = bssl_crypto::rand_array();
    let subject = Name::from_str("CN=Android Protected Virtual Machine Key")?.to_der()?;
    let rkp_cert = Certificate::from_der(&params.remotely_provisioned_cert)?;

    let vm_payload_components = client_vm_dice_chain.microdroid_payload_components()?;
    let vm_tenant_components = if cfg!(advance_multitenancy) {
        client_vm_dice_chain.microdroid_tenant_components()
    } else {
        Ok(Vec::new())
    }?;

    fn to_vm_components(
        components: &[crate::dice::SubComponent],
    ) -> der::Result<Vec<cert::VmComponent<'_>>> {
        components.iter().map(cert::VmComponent::new).collect()
    }

    let vm_payload_components = to_vm_components(&vm_payload_components)?;
    let _vm_tenant_components = to_vm_components(&vm_tenant_components)?;

    info!("The client VM DICE chain validation succeeded. Beginning to generate the certificate.");
    let attestation_ext = {
        #[cfg(advance_multitenancy)]
        {
            cert::AttestationExtension::new(
                &csr_payload.challenge,
                client_vm_dice_chain.all_entries_are_secure(),
                vm_payload_components,
                _vm_tenant_components,
            )
        }
        #[cfg(not(advance_multitenancy))]
        {
            cert::AttestationExtension::new(
                &csr_payload.challenge,
                client_vm_dice_chain.all_entries_are_secure(),
                vm_payload_components,
            )
        }
    }
    .to_der()?;
    let tbs_cert = cert::build_tbs_certificate(
        &serial_number,
        rkp_cert.tbs_certificate.subject,
        Name::from_der(&subject)?,
        rkp_cert.tbs_certificate.validity,
        subject_public_key_info.as_ref(),
        &attestation_ext,
    )?;

    // Signs the TBSCertificate and builds the Certificate.
    // The two private key structs below will be zeroed out on drop.
    let private_key =
        decrypt_private_key(&params.remotely_provisioned_key_blob, ops).map_err(|e| {
            error!("Failed to decrypt the remotely provisioned key blob: {e}");
            RequestProcessingError::FailedToDecryptKeyBlob
        })?;
    let ec_private_key = ecdsa::PrivateKey::<P256>::from_der_ec_private_key(private_key.as_slice())
        .ok_or(RequestProcessingError::DerError)?;
    let signature = ec_private_key.sign(&tbs_cert.to_der()?);
    let certificate = cert::build_certificate(tbs_cert, &signature)?;
    Ok(certificate.to_der()?)
}

fn p256_cose_public_key_from_slice(key: &[u8]) -> Result<ecdsa::PublicKey<P256>> {
    let key = CoseKey::from_slice(key)?;
    if key.alg != Some(Algorithm::Assigned(iana::Algorithm::ES256)) {
        error!("Invalid algorithm in COSE key {:?}", key.alg);
        return Err(RequestProcessingError::InvalidDiceChain);
    };
    let crv = get_label_value(&key, Label::Int(iana::Ec2KeyParameter::Crv.to_i64()))?;
    if crv != &Value::from(iana::EllipticCurve::P_256.to_i64()) {
        error!("Invalid curve in COSE key {:?}", key.alg);
        return Err(RequestProcessingError::InvalidDiceChain);
    }
    let sec1 = key.to_sec1_octet_string()?;
    ecdsa::PublicKey::<P256>::from_x962_uncompressed(&sec1)
        .ok_or(RequestProcessingError::InvalidDiceChain)
}

/// Validates the client VM DICE chain against the Reference VM DICE chain.
///
/// Returns the valid `ClientVmDiceChain` if the validation succeeds.
fn validate_client_vm_dice_chain(
    client_vm_dice_chain_bytes: &[u8],
    ops: &impl AttestationOps,
) -> Result<ClientVmDiceChain> {
    let reference_chain =
        parse_value_array(ops.reference_vm_dice_chain()?, "reference_vm_dice_chain")?;

    let suffix_len = ops.reference_vm_dice_chain_suffix_len();
    let common_len = reference_chain.len().checked_sub(suffix_len).ok_or_else(|| {
        error!(
            "Reference VM DICE chain too short for suffix. Chain: {}, Suffix: {}",
            reference_chain.len(),
            suffix_len
        );
        RequestProcessingError::InternalError
    })?;
    if common_len < 2 {
        error!(
            "Common DICE chain prefix too short. Must contain at least 2 entries \
             (Root Key and a cert describing pvmfw). Got: {}",
            common_len
        );
        return Err(RequestProcessingError::InternalError);
    }

    let client_chain_values =
        parse_value_array(client_vm_dice_chain_bytes, "client_vm_dice_chain")?;

    validate_common_prefix(&client_chain_values, &reference_chain[..common_len])?;

    let client_vm_dice_chain = ClientVmDiceChain::validate_signatures_and_parse_dice_chain(
        client_chain_values,
        common_len,
    )?;

    let reference_authority_entry = reference_chain.get(common_len).ok_or_else(|| {
        error!("Reference chain missing authority entry at index {}", common_len);
        RequestProcessingError::InternalError
    })?;
    let reference_authority_payload =
        DiceChainEntryPayload::from_cbor_value_unchecked(reference_authority_entry.clone())?;
    ensure_same_authority(client_vm_dice_chain.kernel(), &reference_authority_payload)?;

    if cfg!(validate_client_vm_using_dice_info) {
        validate_kernel_dice_info(&client_vm_dice_chain)?;
    } else {
        validate_kernel_code_hash(&client_vm_dice_chain)?;
    }

    ops.validate_vm(&client_vm_dice_chain)?;

    info!("The client VM DICE chain validation succeeded");
    Ok(client_vm_dice_chain)
}

/// Verifies that the `actual` entry acts under the same authority as the `expected` entry.
fn ensure_same_authority(
    actual: &DiceChainEntryPayload,
    expected: &DiceChainEntryPayload,
) -> Result<()> {
    if actual.authority_hash != expected.authority_hash {
        error!(
            "Authority hash mismatch.\n\
             Expected: {:x?}\n\
             Actual:   {:x?}",
            expected.authority_hash, actual.authority_hash
        );
        return Err(RequestProcessingError::InvalidDiceChain);
    }
    Ok(())
}

/// Validates that the kernel code hash in the Client VM DICE chain matches the code hashes
/// embedded during the build time.
fn validate_kernel_code_hash(dice_chain: &ClientVmDiceChain) -> Result<()> {
    fn matches_any_kernel_code_hash(actual_code_hash: &[u8], is_debug: bool) -> bool {
        for os_hash in &microdroid_kernel_hashes::OS_HASHES {
            let mut code_hash = [0u8; microdroid_kernel_hashes::HASH_SIZE * 2];
            code_hash[0..microdroid_kernel_hashes::HASH_SIZE].copy_from_slice(&os_hash.kernel);
            if is_debug {
                code_hash[microdroid_kernel_hashes::HASH_SIZE..]
                    .copy_from_slice(&os_hash.initrd_debug);
            } else {
                code_hash[microdroid_kernel_hashes::HASH_SIZE..]
                    .copy_from_slice(&os_hash.initrd_normal);
            }
            if Sha512::hash(&code_hash) == actual_code_hash {
                return true;
            }
        }
        false
    }

    let kernel = dice_chain.kernel();
    if matches_any_kernel_code_hash(&kernel.code_hash, /* is_debug= */ false) {
        return Ok(());
    }
    if matches_any_kernel_code_hash(&kernel.code_hash, /* is_debug= */ true) {
        if dice_chain.all_entries_are_secure() {
            error!("The Microdroid kernel has debug initrd but the DICE chain is secure");
            return Err(RequestProcessingError::InvalidDiceChain);
        }
        return Ok(());
    }
    error!("The kernel code hash in the Client VM DICE chain does not match any expected values");
    Err(RequestProcessingError::InvalidDiceChain)
}

fn validate_kernel_dice_info(dice_chain: &ClientVmDiceChain) -> Result<()> {
    const AUTHORIZED_KERNEL_COMPONENT_NAMES: &[&str; 1] = &[
        "vm_entry", // Microdroid VM
    ];

    let kernel = dice_chain.kernel();

    // 1. Check if the kernel component name is present and authorized.
    let kernel_component_name = kernel.component_name().ok_or_else(|| {
        error!("Kernel component name missing in DICE chain");
        RequestProcessingError::InvalidDiceChain
    })?;
    if !AUTHORIZED_KERNEL_COMPONENT_NAMES.contains(&kernel_component_name) {
        error!("Unauthorized kernel component name: {}", kernel_component_name);
        return Err(RequestProcessingError::InvalidDiceChain);
    }

    // 2. Enforce Anti-Rollback policy: The kernel's security version must not be older than the
    //    platform's security patch timestamp.
    let kernel_security_version = kernel.security_version().ok_or_else(|| {
        error!("Kernel security version missing in DICE chain");
        RequestProcessingError::InvalidDiceChain
    })?;

    if kernel_security_version < platform_security_patch_timestamp::TIMESTAMP {
        error!(
            "Kernel security version too old. Kernel version: {}, Platform version: {}",
            kernel_security_version,
            platform_security_patch_timestamp::TIMESTAMP
        );
        return Err(RequestProcessingError::InvalidDiceChain);
    }

    Ok(())
}

fn validate_common_prefix(client_chain: &[Value], common_chain: &[Value]) -> Result<()> {
    if !client_chain.starts_with(common_chain) {
        error!("Client VM DICE chain does not match the expected common chain prefix");
        return Err(RequestProcessingError::InvalidDiceChain);
    }
    Ok(())
}
