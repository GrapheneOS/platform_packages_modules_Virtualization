/*
 * Copyright (C) 2021 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use anyhow::Result;
use apkverify::{
    extract_signed_data, get_apk_digest, testing::assert_contains, verify, SignatureAlgorithmID,
};
use apkzip::zip_sections;
use byteorder::{LittleEndian, ReadBytesExt};
use log::info;
use openssl::x509::X509;
use std::fmt::Write;
use std::io::{Seek, SeekFrom};
use std::{fs, matches, path::Path};

const KEY_NAMES_DSA: &[&str] = &["1024", "2048", "3072"];
const KEY_NAMES_ECDSA: &[&str] = &["p256", "p384", "p521"];
const KEY_NAMES_RSA: &[&str] = &["1024", "2048", "3072", "4096", "8192"];

const SDK_INT: u32 = 31;

/// Make sure any logging from the code under test ends up in logcat.
fn setup() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("apkverify_test")
            .with_max_level(log::LevelFilter::Info),
    );
    info!("Test starting");
}

#[test]
fn test_zip_sections_with_apk() {
    let mut reader = fs::File::open("tests/data/v3-only-with-stamp.apk").unwrap();
    let sections = zip_sections(&mut reader).unwrap();

    // Checks Central directory.
    assert_eq!(
        sections.central_directory_offset + sections.central_directory_size,
        sections.eocd_offset
    );

    // Checks EOCD.
    const EOCD_SIGNATURE: u32 = 0x06054b50;

    reader.seek(SeekFrom::Start(sections.eocd_offset as u64)).unwrap();
    assert_eq!(reader.read_u32::<LittleEndian>().unwrap(), EOCD_SIGNATURE);
    assert_eq!(
        reader.metadata().unwrap().len(),
        (sections.eocd_offset + sections.eocd_size) as u64
    );
}

#[test]
fn test_verify_truncated_cd() {
    setup();
    use zip::result::ZipError;
    let res = verify("tests/data/v2-only-truncated-cd.apk", SDK_INT);
    // TODO(b/190343842): consider making a helper for err assertion
    assert!(matches!(
        res.unwrap_err().root_cause().downcast_ref::<ZipError>().unwrap(),
        ZipError::InvalidArchive(_),
    ));
}

#[test]
fn apex_signed_with_v3_rsa_pkcs1_sha512_is_valid() {
    setup();
    validate_apk("tests/data/test.apex", SignatureAlgorithmID::RsaPkcs1V15WithSha512);
}

#[test]
fn apks_signed_with_v3_dsa_sha256_are_not_supported() {
    setup();
    for key_name in KEY_NAMES_DSA.iter() {
        let res = verify(format!("tests/data/v3-only-with-dsa-sha256-{key_name}.apk"), SDK_INT);
        assert!(res.is_err(), "DSA algorithm is not supported for verification. See b/197052981.");
        assert_contains(&res.unwrap_err().to_string(), "No supported APK signatures found");
    }
}

#[test]
fn apks_signed_with_v3_ecdsa_sha256_are_valid() {
    setup();
    for key_name in KEY_NAMES_ECDSA.iter() {
        validate_apk(
            format!("tests/data/v3-only-with-ecdsa-sha256-{key_name}.apk"),
            SignatureAlgorithmID::EcdsaWithSha256,
        );
    }
}

#[test]
fn apks_signed_with_v3_ecdsa_sha512_are_valid() {
    setup();
    for key_name in KEY_NAMES_ECDSA.iter() {
        validate_apk(
            format!("tests/data/v3-only-with-ecdsa-sha512-{key_name}.apk"),
            SignatureAlgorithmID::EcdsaWithSha512,
        );
    }
}

#[test]
fn apks_signed_with_v3_rsa_pkcs1_sha256_are_valid() {
    setup();
    for key_name in KEY_NAMES_RSA.iter() {
        validate_apk(
            format!("tests/data/v3-only-with-rsa-pkcs1-sha256-{key_name}.apk"),
            SignatureAlgorithmID::RsaPkcs1V15WithSha256,
        );
    }
}

#[test]
fn apks_signed_with_v3_rsa_pkcs1_sha512_are_valid() {
    setup();
    for key_name in KEY_NAMES_RSA.iter() {
        validate_apk(
            format!("tests/data/v3-only-with-rsa-pkcs1-sha512-{key_name}.apk"),
            SignatureAlgorithmID::RsaPkcs1V15WithSha512,
        );
    }
}

#[test]
fn test_verify_v3_sig_min_max_sdk() {
    setup();
    // The Signer for this APK has min_sdk=24, max_sdk=32.
    let path = "tests/data/v31-rsa-2048_2-tgt-33-1-tgt-28.apk";

    let res = verify(path, 23);
    assert!(res.is_err());
    assert_contains(&res.unwrap_err().to_string(), "0 signers found");

    let res = verify(path, 24);
    assert!(res.is_ok());

    let res = verify(path, 32);
    assert!(res.is_ok());

    let res = verify(path, 33);
    assert!(res.is_err());
    assert_contains(&res.unwrap_err().to_string(), "0 signers found");
}

#[test]
fn test_verify_v3_sig_does_not_verify() {
    setup();
    let path_list = [
        "tests/data/v3-only-with-ecdsa-sha512-p521-sig-does-not-verify.apk",
        "tests/data/v3-only-with-rsa-pkcs1-sha256-3072-sig-does-not-verify.apk",
    ];
    for path in path_list.iter() {
        let res = verify(path, SDK_INT);
        assert!(res.is_err());
        assert_contains(&res.unwrap_err().to_string(), "Signature is invalid");
    }
}

#[test]
fn test_verify_v3_digest_mismatch() {
    setup();
    let res = verify("tests/data/v3-only-with-rsa-pkcs1-sha512-8192-digest-mismatch.apk", SDK_INT);
    assert!(res.is_err());
    assert_contains(&res.unwrap_err().to_string(), "Digest mismatch");
}

#[test]
fn test_verify_v3_wrong_apk_sig_block_magic() {
    setup();
    let res =
        verify("tests/data/v3-only-with-ecdsa-sha512-p384-wrong-apk-sig-block-magic.apk", SDK_INT);
    assert!(res.is_err());
    assert_contains(&res.unwrap_err().to_string(), "No APK Signing Block");
}

#[test]
fn test_verify_v3_apk_sig_block_size_mismatch() {
    setup();
    let res = verify(
        "tests/data/v3-only-with-rsa-pkcs1-sha512-4096-apk-sig-block-size-mismatch.apk",
        SDK_INT,
    );
    assert!(res.is_err());
    assert_contains(
        &res.unwrap_err().to_string(),
        "APK Signing Block sizes in header and footer do not match",
    );
}

#[test]
fn test_verify_v3_cert_and_public_key_mismatch() {
    setup();
    let res = verify("tests/data/v3-only-cert-and-public-key-mismatch.apk", SDK_INT);
    assert!(res.is_err());
    assert_contains(&res.unwrap_err().to_string(), "Public key mismatch");
}

#[test]
fn test_verify_v3_empty() {
    setup();
    let res = verify("tests/data/v3-only-empty.apk", SDK_INT);
    assert!(res.is_err());
    assert_contains(&res.unwrap_err().to_string(), "APK too small for APK Signing Block");
}

#[test]
fn test_verify_v3_no_certs_in_sig() {
    setup();
    let res = verify("tests/data/v3-only-no-certs-in-sig.apk", SDK_INT);
    assert!(res.is_err());
    assert_contains(&res.unwrap_err().to_string(), "No certificates listed");
}

#[test]
fn test_verify_v3_no_supported_sig_algs() {
    setup();
    let res = verify("tests/data/v3-only-no-supported-sig-algs.apk", SDK_INT);
    assert!(res.is_err());
    assert_contains(&res.unwrap_err().to_string(), "No supported APK signatures found");
}

#[test]
fn test_verify_v3_signatures_and_digests_block_mismatch() {
    setup();
    let res = verify("tests/data/v3-only-signatures-and-digests-block-mismatch.apk", SDK_INT);
    assert!(res.is_err());
    assert_contains(
        &res.unwrap_err().to_string(),
        "Signature algorithms don't match between digests and signatures records",
    );
}

#[test]
fn apk_signed_with_v3_unknown_additional_attr_is_valid() {
    setup();
    validate_apk(
        "tests/data/v3-only-unknown-additional-attr.apk",
        SignatureAlgorithmID::RsaPkcs1V15WithSha256,
    );
}

#[test]
fn apk_signed_with_v3_unknown_pair_in_apk_sig_block_is_valid() {
    setup();
    validate_apk(
        "tests/data/v3-only-unknown-pair-in-apk-sig-block.apk",
        SignatureAlgorithmID::RsaPkcs1V15WithSha256,
    );
}

#[test]
fn apk_signed_with_v3_ignorable_unsupported_sig_algs_is_valid() {
    setup();
    validate_apk(
        "tests/data/v3-only-with-ignorable-unsupported-sig-algs.apk",
        SignatureAlgorithmID::RsaPkcs1V15WithSha256,
    );
}

#[test]
fn apk_signed_with_v3_stamp_is_valid() {
    setup();
    validate_apk("tests/data/v3-only-with-stamp.apk", SignatureAlgorithmID::EcdsaWithSha256);
}

fn validate_apk<P: AsRef<Path>>(apk_path: P, expected_algorithm_id: SignatureAlgorithmID) {
    validate_apk_public_key(&apk_path);
    validate_apk_digest(&apk_path, expected_algorithm_id);
}

/// Validates that the following public keys are equal:
/// * public key from verification
/// * public key extracted from apk without verification
/// * expected public key from the corresponding .der file
fn validate_apk_public_key<P: AsRef<Path>>(apk_path: P) {
    let signed_data_from_verification =
        verify(&apk_path, SDK_INT).expect("Error in verification result");
    let cert_from_verification = signed_data_from_verification.first_certificate_der().unwrap();
    let public_key_from_verification = public_key_der_from_cert(cert_from_verification).unwrap();

    let expected_public_key_path = format!("{}.der", apk_path.as_ref().to_str().unwrap());
    assert_bytes_eq_to_data_in_file(&public_key_from_verification, expected_public_key_path);

    let signed_data_from_apk = extract_signed_data(&apk_path, SDK_INT)
        .expect("Error when extracting signed data from apk");
    let cert_from_apk = signed_data_from_apk.first_certificate_der().unwrap();
    // If the two certficiates are byte for byte identical (which they should be), then so are
    // the public keys embedded in them.
    assert_eq!(
        cert_from_verification, cert_from_apk,
        "Certificate extracted directly from apk does not match the certificate from verification."
    );
}

fn public_key_der_from_cert(cert_der: &[u8]) -> Result<Vec<u8>> {
    let cert = X509::from_der(cert_der)?;
    Ok(cert.public_key()?.public_key_to_der()?)
}

/// Validates that the following apk_digest are equal:
/// * apk_digest directly extracted from apk without computation
/// * computed apk_digest
/// * expected apk digest from the corresponding .apk_digest file
fn validate_apk_digest<P: AsRef<Path>>(apk_path: P, expected_algorithm_id: SignatureAlgorithmID) {
    let apk = fs::File::open(&apk_path).expect("Unabled to open apk file");

    let (verified_algorithm_id, verified_digest) =
        get_apk_digest(&apk, SDK_INT, /* verify= */ true)
            .expect("Error when extracting apk digest with verification.");

    assert_eq!(expected_algorithm_id, verified_algorithm_id);
    let expected_digest_path = format!("{}.apk_digest", apk_path.as_ref().to_str().unwrap());
    assert_bytes_eq_to_data_in_file(&verified_digest, expected_digest_path);

    let (unverified_algorithm_id, unverified_digest) =
        get_apk_digest(&apk, SDK_INT, /* verify= */ false)
            .expect("Error when extracting apk digest without verification.");
    assert_eq!(expected_algorithm_id, unverified_algorithm_id);
    assert_eq!(verified_digest, unverified_digest);
}

fn assert_bytes_eq_to_data_in_file<P: AsRef<Path> + std::fmt::Display>(
    bytes_data: &[u8],
    expected_data_path: P,
) {
    assert!(
        fs::metadata(&expected_data_path).is_ok(),
        "File does not exist. You can re-create it with:\n$ echo -en {} > {}\n",
        bytes_data.iter().fold(String::new(), |mut output, b| {
            let _ = write!(output, "\\\\x{b:02x}");
            output
        }),
        expected_data_path
    );
    let expected_data = fs::read(&expected_data_path).unwrap();
    assert_eq!(
        expected_data, bytes_data,
        "Actual data does not match the data from: {expected_data_path}"
    );
}
