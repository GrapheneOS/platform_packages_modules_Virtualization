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
use coset::{
    iana, CborSerializable, CoseKey, CoseKeyBuilder, CoseSign, CoseSignBuilder, CoseSignature,
    CoseSignatureBuilder, HeaderBuilder,
};
use diced_open_dice::{
    derive_cdi_leaf_priv, sign, DiceArtifacts, PrivateKey, DICE_COSE_KEY_ALG_VALUE,
};
use openssl::{
    bn::{BigNum, BigNumContext},
    ec::{EcGroup, EcKey, EcKeyRef},
    ecdsa::EcdsaSig,
    nid::Nid,
    pkey::Private,
    sha::sha256,
};
use service_vm_comm::{Csr, CsrPayload};
use zeroize::Zeroizing;

/// Key parameters for the attestation key.
///
/// See libs/libservice_vm_comm/client_vm_csr.cddl for more information about the attestation key.
const ATTESTATION_KEY_NID: Nid = Nid::X9_62_PRIME256V1; // NIST P-256 curve
const ATTESTATION_KEY_ALGO: iana::Algorithm = iana::Algorithm::ES256;
const ATTESTATION_KEY_CURVE: iana::EllipticCurve = iana::EllipticCurve::P_256;
const ATTESTATION_KEY_AFFINE_COORDINATE_SIZE: i32 = 32;

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
    dice_artifacts: &dyn DiceArtifacts,
) -> Result<ClientVmAttestationData> {
    let group = EcGroup::from_curve_name(ATTESTATION_KEY_NID)?;
    let attestation_key = EcKey::generate(&group)?;

    let csr = build_csr(challenge, attestation_key.as_ref(), dice_artifacts)?;
    let private_key = attestation_key.private_key_to_der()?;
    Ok(ClientVmAttestationData { private_key: Zeroizing::new(private_key), csr })
}

fn build_csr(
    challenge: &[u8],
    attestation_key: &EcKeyRef<Private>,
    dice_artifacts: &dyn DiceArtifacts,
) -> Result<Csr> {
    // Builds CSR Payload to be signed.
    let public_key =
        to_cose_public_key(attestation_key)?.to_vec().context("Failed to serialize public key")?;
    let csr_payload = CsrPayload { public_key, challenge: challenge.to_vec() };
    let csr_payload = csr_payload.into_cbor_vec()?;

    // Builds signed CSR Payload.
    let cdi_leaf_priv = derive_cdi_leaf_priv(dice_artifacts)?;
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
    attestation_key: &EcKeyRef<Private>,
) -> Result<CoseSign> {
    let dice_key_alg = cbor_util::dice_cose_key_alg(DICE_COSE_KEY_ALG_VALUE)?;
    let cdi_leaf_sig_headers = build_signature_headers(dice_key_alg);
    let attestation_key_sig_headers = build_signature_headers(ATTESTATION_KEY_ALGO);
    let aad = &[];
    let signed_data = CoseSignBuilder::new()
        .payload(payload)
        .try_add_created_signature(cdi_leaf_sig_headers, aad, |message| {
            sign(message, cdi_leaf_priv.as_array()).map(|v| v.to_vec())
        })?
        .try_add_created_signature(attestation_key_sig_headers, aad, |message| {
            ecdsa_sign_cose(message, attestation_key)
        })?
        .build();
    Ok(signed_data)
}

/// Builds a signature with headers filled with the provided algorithm.
/// The signature data will be filled later when building the signed data.
fn build_signature_headers(alg: iana::Algorithm) -> CoseSignature {
    let protected = HeaderBuilder::new().algorithm(alg).build();
    CoseSignatureBuilder::new().protected(protected).build()
}

fn ecdsa_sign_cose(message: &[u8], key: &EcKeyRef<Private>) -> Result<Vec<u8>> {
    let digest = sha256(message);
    // Passes the digest to `ECDSA_do_sign` as recommended in the spec:
    // https://commondatastorage.googleapis.com/chromium-boringssl-docs/ecdsa.h.html#ECDSA_do_sign
    let sig = EcdsaSig::sign::<Private>(&digest, key)?;
    ecdsa_sig_to_cose(&sig)
}

fn ecdsa_sig_to_cose(signature: &EcdsaSig) -> Result<Vec<u8>> {
    let mut result = signature.r().to_vec_padded(ATTESTATION_KEY_AFFINE_COORDINATE_SIZE)?;
    result.extend_from_slice(&signature.s().to_vec_padded(ATTESTATION_KEY_AFFINE_COORDINATE_SIZE)?);
    Ok(result)
}

fn get_affine_coordinates(key: &EcKeyRef<Private>) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut ctx = BigNumContext::new()?;
    let mut x = BigNum::new()?;
    let mut y = BigNum::new()?;
    key.public_key().affine_coordinates_gfp(key.group(), &mut x, &mut y, &mut ctx)?;
    let x = x.to_vec_padded(ATTESTATION_KEY_AFFINE_COORDINATE_SIZE)?;
    let y = y.to_vec_padded(ATTESTATION_KEY_AFFINE_COORDINATE_SIZE)?;
    Ok((x, y))
}

fn to_cose_public_key(key: &EcKeyRef<Private>) -> Result<CoseKey> {
    let (x, y) = get_affine_coordinates(key)?;
    Ok(CoseKeyBuilder::new_ec2_pub_key(ATTESTATION_KEY_CURVE, x, y)
        .algorithm(ATTESTATION_KEY_ALGO)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use ciborium::Value;
    use coset::{iana::EnumI64, Label};
    use hwtrust::{dice, session::Session};
    use openssl::pkey::Public;

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
        let ec_private_key = EcKey::private_key_from_der(&private_key)?;
        let cose_sign = CoseSign::from_slice(&csr.signed_csr_payload).unwrap();
        let aad = &[];

        // Checks CSR payload.
        let csr_payload =
            cose_sign.payload.as_ref().and_then(|v| CsrPayload::from_cbor_slice(v).ok()).unwrap();
        let public_key = to_cose_public_key(&ec_private_key)?.to_vec().unwrap();
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
                ecdsa_verify_cose(signature, message, &ec_public_key)
            })
            .context("Verifying attestation key signature")?;

        // Verifies that private key and the public key form a valid key pair.
        let message = b"test message";
        let signature = ecdsa_sign_cose(message, &ec_private_key)?;
        ecdsa_verify_cose(&signature, message, &ec_public_key)
            .context("Verifying signature with attested key")?;

        Ok(())
    }

    fn ecdsa_verify_cose(
        signature: &[u8],
        message: &[u8],
        ec_public_key: &EcKeyRef<Public>,
    ) -> Result<()> {
        let coord_bytes = signature.len() / 2;
        assert_eq!(signature.len(), coord_bytes * 2);

        let r = BigNum::from_slice(&signature[..coord_bytes])?;
        let s = BigNum::from_slice(&signature[coord_bytes..])?;
        let sig = EcdsaSig::from_private_components(r, s)?;
        let digest = sha256(message);
        if sig.verify(&digest, ec_public_key)? {
            Ok(())
        } else {
            bail!("Signature does not match")
        }
    }

    fn to_ec_public_key(cose_key: &CoseKey) -> Result<EcKey<Public>> {
        check_ec_key_params(cose_key)?;
        let group = EcGroup::from_curve_name(ATTESTATION_KEY_NID)?;
        let x = get_label_value_as_bignum(cose_key, Label::Int(iana::Ec2KeyParameter::X.to_i64()))?;
        let y = get_label_value_as_bignum(cose_key, Label::Int(iana::Ec2KeyParameter::Y.to_i64()))?;
        let key = EcKey::from_public_key_affine_coordinates(&group, &x, &y)?;
        key.check_key()?;
        Ok(key)
    }

    fn check_ec_key_params(cose_key: &CoseKey) -> Result<()> {
        assert_eq!(coset::KeyType::Assigned(iana::KeyType::EC2), cose_key.kty);
        assert_eq!(Some(coset::Algorithm::Assigned(ATTESTATION_KEY_ALGO)), cose_key.alg);
        let crv = get_label_value(cose_key, Label::Int(iana::Ec2KeyParameter::Crv.to_i64()))?;
        assert_eq!(&Value::from(ATTESTATION_KEY_CURVE.to_i64()), crv);
        Ok(())
    }

    fn get_label_value_as_bignum(key: &CoseKey, label: Label) -> Result<BigNum> {
        get_label_value(key, label)?
            .as_bytes()
            .map(|v| BigNum::from_slice(&v[..]).unwrap())
            .ok_or_else(|| anyhow!("Value not a bstr."))
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
