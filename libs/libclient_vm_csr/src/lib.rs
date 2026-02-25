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

//! Generate the attestation key and CSR for client VM in the remote
//! attestation.

use anyhow::{anyhow, Context, Result};
use bssl_crypto::{ec::P256, ecdsa};
use coset::{
    iana, CborSerializable, CoseKey, CoseKeyBuilder, CoseSign, CoseSignBuilder, CoseSignature,
    CoseSignatureBuilder, HeaderBuilder,
};
use diced_open_dice::{derive_cdi_leaf_priv, sign, DiceArtifacts, PrivateKey, VM_KEY_ALGORITHM};
use service_vm_comm::{Csr, CsrPayload};
use zeroize::Zeroizing;

/// Key parameters for the attestation key.
///
/// See libs/libservice_vm_comm/client_vm_csr.cddl for more information about the attestation key.
const ATTESTATION_KEY_ALGO: iana::Algorithm = iana::Algorithm::ES256;
const ATTESTATION_KEY_CURVE: iana::EllipticCurve = iana::EllipticCurve::P_256;

/// Represents the output of generating the attestation key and CSR for the client VM.
pub struct ClientVmAttestationData {
    /// DER-encoded ECPrivateKey to be attested.
    pub private_key: Zeroizing<Vec<u8>>,

    /// CSR containing client VM information and the public key corresponding to the
    /// private key to be attested.
    pub csr: Csr,
}

/// Generates the attestation key and CSR including the public key to be attested for the
/// client VM in remote attestation.
pub fn generate_attestation_key_and_csr(
    challenge: &[u8],
    dice_artifacts: &impl DiceArtifacts,
) -> Result<ClientVmAttestationData> {
    let attestation_key = ecdsa::PrivateKey::<P256>::generate();
    let csr = build_csr(challenge, &attestation_key, dice_artifacts)?;
    let private_key = attestation_key.to_der_ec_private_key().as_ref().to_vec();
    Ok(ClientVmAttestationData { private_key: Zeroizing::new(private_key), csr })
}

fn build_csr(
    challenge: &[u8],
    attestation_key: &ecdsa::PrivateKey<P256>,
    dice_artifacts: &impl DiceArtifacts,
) -> Result<Csr> {
    // Builds CSR Payload to be signed.
    let public_key = to_cose_public_key(&attestation_key.to_public_key())?
        .to_vec()
        .context("Failed to serialize public key")?;
    let csr_payload = CsrPayload { public_key, challenge: challenge.to_vec() };
    let csr_payload = csr_payload.into_cbor_vec()?;

    // Builds signed CSR Payload.
    let cdi_leaf_priv = derive_cdi_leaf_priv(None, dice_artifacts)?;
    let signed_csr_payload = build_signed_data(csr_payload, &cdi_leaf_priv, attestation_key)?
        .to_vec()
        .context("Failed to serialize signed CSR payload")?;

    // Builds CSR.
    let dice_cert_chain = dice_artifacts.bcc().ok_or(anyhow!("bcc is none"))?.to_vec();
    Ok(Csr { dice_cert_chain, signed_csr_payload })
}

fn build_signed_data(
    payload: Vec<u8>,
    cdi_leaf_priv: &PrivateKey,
    attestation_key: &ecdsa::PrivateKey<P256>,
) -> Result<CoseSign> {
    let cdi_leaf_sig_headers = build_signature_headers(VM_KEY_ALGORITHM.into());
    let attestation_key_sig_headers = build_signature_headers(ATTESTATION_KEY_ALGO);
    let aad = &[];
    let signed_data = CoseSignBuilder::new()
        .payload(payload)
        .try_add_created_signature(cdi_leaf_sig_headers, aad, |message| {
            sign(message, cdi_leaf_priv.as_array()).map(|v| v.to_vec())
        })?
        .add_created_signature(attestation_key_sig_headers, aad, |message| {
            attestation_key.sign_p1363(message)
        })
        .build();
    Ok(signed_data)
}

/// Builds a signature with headers filled with the provided algorithm.
/// The signature data will be filled later when building the signed data.
fn build_signature_headers(alg: iana::Algorithm) -> CoseSignature {
    let protected = HeaderBuilder::new().algorithm(alg).build();
    CoseSignatureBuilder::new().protected(protected).build()
}

fn to_cose_public_key(key: &ecdsa::PublicKey<P256>) -> Result<CoseKey> {
    let sec1 = key.to_x962_uncompressed();
    Ok(CoseKeyBuilder::new_ec2_pub_key_sec1_octet_string(ATTESTATION_KEY_CURVE, sec1.as_ref())
        .context("Failed to build COSE key")?
        .algorithm(ATTESTATION_KEY_ALGO)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ciborium::Value;
    use coset::{iana::EnumI64, Label};
    use hwtrust::{dice, session::Session};

    /// The following data was generated randomly with urandom.
    const CHALLENGE: [u8; 16] = [
        0xb3, 0x66, 0xfa, 0x72, 0x92, 0x32, 0x2c, 0xd4, 0x99, 0xcb, 0x00, 0x1f, 0x0e, 0xe0, 0xc7,
        0x41,
    ];

    #[test]
    fn csr_and_private_key_have_correct_format() -> Result<()> {
        let dice_artifacts = diced_sample_inputs::make_sample_bcc_and_cdis()?;

        let ClientVmAttestationData { private_key, csr } =
            generate_attestation_key_and_csr(&CHALLENGE, &dice_artifacts)?;
        let ec_private_key =
            ecdsa::PrivateKey::<P256>::from_der_ec_private_key(&private_key).unwrap();
        let cose_sign = CoseSign::from_slice(&csr.signed_csr_payload).unwrap();
        let aad = &[];

        // Checks CSR payload.
        let csr_payload =
            cose_sign.payload.as_ref().and_then(|v| CsrPayload::from_cbor_slice(v).ok()).unwrap();
        let public_key = to_cose_public_key(&ec_private_key.to_public_key())?.to_vec().unwrap();
        let expected_csr_payload = CsrPayload { challenge: CHALLENGE.to_vec(), public_key };
        assert_eq!(expected_csr_payload, csr_payload);

        // Checks the first signature is signed with CDI_Leaf_Priv.
        let session = Session::default();
        let chain = dice::Chain::from_cbor(&session, &csr.dice_cert_chain)?;
        let public_key = chain.leaf().subject_public_key();
        cose_sign
            .verify_signature(0, aad, |signature, message| public_key.verify(signature, message))
            .context("Verifying CDI_Leaf_Priv signature")?;

        // Checks the second signature is signed with attestation key.
        let attestation_public_key = CoseKey::from_slice(&csr_payload.public_key).unwrap();
        let ec_public_key = to_ec_public_key(&attestation_public_key)?;
        cose_sign
            .verify_signature(1, aad, |signature, message| {
                ec_public_key.verify_p1363(message, signature)
            })
            .map_err(|_| anyhow!("Verifying attestation key signature"))?;

        // Verifies that private key and the public key form a valid key pair.
        let message = b"test message";
        let signature = ec_private_key.sign(message);
        ec_public_key
            .verify(message, &signature)
            .map_err(|_| anyhow!("Verifying signature with attested key"))?;

        Ok(())
    }

    fn to_ec_public_key(cose_key: &CoseKey) -> Result<ecdsa::PublicKey<P256>> {
        assert_eq!(coset::KeyType::Assigned(iana::KeyType::EC2), cose_key.kty);
        assert_eq!(Some(coset::Algorithm::Assigned(ATTESTATION_KEY_ALGO)), cose_key.alg);
        let crv = get_label_value(cose_key, Label::Int(iana::Ec2KeyParameter::Crv.to_i64()))?;
        assert_eq!(&Value::from(ATTESTATION_KEY_CURVE.to_i64()), crv);
        let sec1 = cose_key.to_sec1_octet_string()?;
        ecdsa::PublicKey::<P256>::from_x962_uncompressed(&sec1)
            .ok_or_else(|| anyhow!("Invalid public key"))
    }

    fn get_label_value(key: &CoseKey, label: Label) -> Result<&Value> {
        Ok(&key
            .params
            .iter()
            .find(|(k, _)| k == &label)
            .ok_or_else(|| anyhow!("Label {:?} not found", label))?
            .1)
    }
}
