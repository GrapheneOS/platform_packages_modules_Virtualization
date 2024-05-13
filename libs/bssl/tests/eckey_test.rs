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

use bssl_avf::{sha256, ApiName, Digester, EcKey, EcdsaError, Error, PKey, Result};
use coset::CborSerializable;
use spki::{
    der::{AnyRef, Decode, Encode},
    AlgorithmIdentifier, ObjectIdentifier, SubjectPublicKeyInfoRef,
};

/// OID value for general-use NIST EC keys held in PKCS#8 and X.509; see RFC 5480 s2.1.1.
const X509_NIST_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");

/// OID value in `AlgorithmIdentifier.parameters` for P-256; see RFC 5480 s2.1.1.1.
const ALGO_PARAM_P256_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");

const MESSAGE1: &[u8] = b"test message 1";
const MESSAGE2: &[u8] = b"test message 2";

#[test]
fn ec_private_key_serialization() -> Result<()> {
    let mut ec_key = EcKey::new_p256()?;
    ec_key.generate_key()?;
    let der_encoded_ec_private_key = ec_key.ec_private_key()?;
    let deserialized_ec_key = EcKey::from_ec_private_key(der_encoded_ec_private_key.as_slice())?;

    assert_eq!(ec_key.cose_public_key()?, deserialized_ec_key.cose_public_key()?);
    Ok(())
}

#[test]
fn subject_public_key_info_serialization() -> Result<()> {
    let mut ec_key = EcKey::new_p256()?;
    ec_key.generate_key()?;
    let pkey: PKey = ec_key.try_into()?;
    let subject_public_key_info = pkey.subject_public_key_info()?;

    let subject_public_key_info =
        SubjectPublicKeyInfoRef::from_der(&subject_public_key_info).unwrap();
    let expected_algorithm = AlgorithmIdentifier {
        oid: X509_NIST_OID,
        parameters: Some(AnyRef::from(&ALGO_PARAM_P256_OID)),
    };
    assert_eq!(expected_algorithm, subject_public_key_info.algorithm);
    assert!(!subject_public_key_info.subject_public_key.to_der().unwrap().is_empty());
    Ok(())
}

#[test]
fn p256_cose_public_key_serialization() -> Result<()> {
    let mut ec_key = EcKey::new_p256()?;
    check_cose_public_key_serialization(&mut ec_key)
}

#[test]
fn p384_cose_public_key_serialization() -> Result<()> {
    let mut ec_key = EcKey::new_p384()?;
    check_cose_public_key_serialization(&mut ec_key)
}

fn check_cose_public_key_serialization(ec_key: &mut EcKey) -> Result<()> {
    ec_key.generate_key()?;
    let cose_key = ec_key.cose_public_key()?;
    let cose_key_data = cose_key.clone().to_vec().unwrap();
    let deserialized_ec_key = EcKey::from_cose_public_key_slice(&cose_key_data)?;

    assert_eq!(cose_key, deserialized_ec_key.cose_public_key()?);
    Ok(())
}

#[test]
fn ecdsa_p256_signing_and_verification_succeed() -> Result<()> {
    let mut ec_key = EcKey::new_p256()?;
    ec_key.generate_key()?;
    let digester = Digester::sha256();
    let digest = digester.digest(MESSAGE1)?;
    assert_eq!(digest, sha256(MESSAGE1)?);

    let signature = ec_key.ecdsa_sign_der(&digest)?;
    ec_key.ecdsa_verify_der(&signature, &digest)?;
    // Building a `PKey` from a temporary `CoseKey` should work as the lifetime
    // of the `PKey` is not tied to the lifetime of the `CoseKey`.
    let pkey = PKey::from_cose_public_key(&ec_key.cose_public_key()?)?;
    pkey.verify(&signature, MESSAGE1, Some(digester))
}

#[test]
fn ecdsa_p384_signing_and_verification_succeed() -> Result<()> {
    let mut ec_key = EcKey::new_p384()?;
    ec_key.generate_key()?;
    let digester = Digester::sha384();
    let digest = digester.digest(MESSAGE1)?;

    let signature = ec_key.ecdsa_sign_der(&digest)?;
    ec_key.ecdsa_verify_der(&signature, &digest)?;
    let pkey = PKey::from_cose_public_key(&ec_key.cose_public_key()?)?;
    pkey.verify(&signature, MESSAGE1, Some(digester))
}

#[test]
fn verifying_ecdsa_p256_signed_with_a_different_key_fails() -> Result<()> {
    let mut ec_key1 = EcKey::new_p256()?;
    ec_key1.generate_key()?;
    let digest = sha256(MESSAGE1)?;
    let signature = ec_key1.ecdsa_sign_der(&digest)?;

    let mut ec_key2 = EcKey::new_p256()?;
    ec_key2.generate_key()?;
    let err = ec_key2.ecdsa_verify_der(&signature, &digest).unwrap_err();
    let expected_err = Error::CallFailed(ApiName::ECDSA_verify, EcdsaError::BadSignature.into());
    assert_eq!(expected_err, err);

    let pkey: PKey = ec_key2.try_into()?;
    let err = pkey.verify(&signature, MESSAGE1, Some(Digester::sha256())).unwrap_err();
    let expected_err =
        Error::CallFailed(ApiName::EVP_DigestVerify, EcdsaError::BadSignature.into());
    assert_eq!(expected_err, err);
    Ok(())
}

#[test]
fn verifying_ecdsa_p256_signed_with_a_different_message_fails() -> Result<()> {
    let mut ec_key = EcKey::new_p256()?;
    ec_key.generate_key()?;
    let digest1 = sha256(MESSAGE1)?;
    let signature = ec_key.ecdsa_sign_der(&digest1)?;
    let digest2 = sha256(MESSAGE2)?;

    let err = ec_key.ecdsa_verify_der(&signature, &digest2).unwrap_err();
    let expected_err = Error::CallFailed(ApiName::ECDSA_verify, EcdsaError::BadSignature.into());
    assert_eq!(expected_err, err);
    Ok(())
}

#[test]
fn ecdsa_cose_signing_and_verification_succeed() -> Result<()> {
    let digest = sha256(MESSAGE1)?;
    let mut ec_key = EcKey::new_p256()?;
    ec_key.generate_key()?;

    let signature = ec_key.ecdsa_sign_cose(&digest)?;
    ec_key.ecdsa_verify_cose(&signature, &digest)?;
    assert_eq!(signature.len(), 64);
    Ok(())
}

#[test]
fn verifying_ecdsa_cose_signed_with_a_different_message_fails() -> Result<()> {
    let digest = sha256(MESSAGE1)?;
    let mut ec_key = EcKey::new_p256()?;
    ec_key.generate_key()?;

    let signature = ec_key.ecdsa_sign_cose(&digest)?;

    let err = ec_key.ecdsa_verify_cose(&signature, &sha256(MESSAGE2)?).unwrap_err();
    let expected_err = Error::CallFailed(ApiName::ECDSA_verify, EcdsaError::BadSignature.into());
    assert_eq!(expected_err, err);
    Ok(())
}

#[test]
fn verifying_ecdsa_cose_signed_as_der_fails() -> Result<()> {
    let digest = sha256(MESSAGE1)?;
    let mut ec_key = EcKey::new_p256()?;
    ec_key.generate_key()?;

    let signature = ec_key.ecdsa_sign_cose(&digest)?;
    let err = ec_key.ecdsa_verify_der(&signature, &digest).unwrap_err();
    let expected_err = Error::CallFailed(ApiName::ECDSA_verify, EcdsaError::BadSignature.into());
    assert_eq!(expected_err, err);
    Ok(())
}
