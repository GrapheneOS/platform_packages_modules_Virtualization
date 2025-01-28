/*
 * Copyright (C) 2023 The Android Open Source Project
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

#[cfg(test)]
mod tests {
    use diced_open_dice::{
        derive_cdi_certificate_id, derive_cdi_private_key_seed, hash, kdf, keypair_from_seed,
        retry_sign_cose_sign1, retry_sign_cose_sign1_with_cdi_leaf_priv, sign, verify,
        DiceArtifacts, PrivateKey, CDI_SIZE, HASH_SIZE, ID_SIZE, PRIVATE_KEY_SEED_SIZE,
    };

    use coset::{CborSerializable, CoseSign1};

    // This test initialization is only required for the trusty test harness.
    #[cfg(feature = "trusty")]
    test::init!();

    #[test]
    fn hash_succeeds() {
        const EXPECTED_HASH: [u8; HASH_SIZE] = [
            0x30, 0x9e, 0xcc, 0x48, 0x9c, 0x12, 0xd6, 0xeb, 0x4c, 0xc4, 0x0f, 0x50, 0xc9, 0x02,
            0xf2, 0xb4, 0xd0, 0xed, 0x77, 0xee, 0x51, 0x1a, 0x7c, 0x7a, 0x9b, 0xcd, 0x3c, 0xa8,
            0x6d, 0x4c, 0xd8, 0x6f, 0x98, 0x9d, 0xd3, 0x5b, 0xc5, 0xff, 0x49, 0x96, 0x70, 0xda,
            0x34, 0x25, 0x5b, 0x45, 0xb0, 0xcf, 0xd8, 0x30, 0xe8, 0x1f, 0x60, 0x5d, 0xcf, 0x7d,
            0xc5, 0x54, 0x2e, 0x93, 0xae, 0x9c, 0xd7, 0x6f,
        ];
        assert_eq!(EXPECTED_HASH, hash(b"hello world").expect("hash failed"));
    }

    #[test]
    fn kdf_succeeds() {
        let mut derived_key = [0u8; PRIVATE_KEY_SEED_SIZE];
        kdf(b"myInitialKeyMaterial", b"mySalt", b"myInfo", &mut derived_key).unwrap();
        const EXPECTED_DERIVED_KEY: [u8; PRIVATE_KEY_SEED_SIZE] = [
            0x91, 0x9b, 0x8d, 0x29, 0xc4, 0x1b, 0x93, 0xd7, 0xeb, 0x09, 0xfa, 0xd7, 0xc9, 0x87,
            0xb0, 0xd1, 0xcc, 0x26, 0xef, 0x07, 0x83, 0x42, 0xcf, 0xa3, 0x45, 0x0a, 0x57, 0xe9,
            0x19, 0x86, 0xef, 0x48,
        ];
        assert_eq!(EXPECTED_DERIVED_KEY, derived_key);
    }

    #[test]
    fn derive_cdi_certificate_id_succeeds() {
        const EXPECTED_ID: [u8; ID_SIZE] = [
            0x7a, 0x36, 0x45, 0x2c, 0x02, 0xf6, 0x2b, 0xec, 0xf9, 0x80, 0x06, 0x75, 0x87, 0xa5,
            0xc1, 0x44, 0x0c, 0xd3, 0xc0, 0x6d,
        ];
        assert_eq!(EXPECTED_ID, derive_cdi_certificate_id(b"MyPubKey").unwrap());
    }

    const EXPECTED_SEED: &[u8] = &[
        0xfa, 0x3c, 0x2f, 0x58, 0x37, 0xf5, 0x8e, 0x96, 0x16, 0x09, 0xf5, 0x22, 0xa1, 0xf1, 0xba,
        0xaa, 0x19, 0x95, 0x01, 0x79, 0x2e, 0x60, 0x56, 0xaf, 0xf6, 0x41, 0xe7, 0xff, 0x48, 0xf5,
        0x3a, 0x08, 0x84, 0x8a, 0x98, 0x85, 0x6d, 0xf5, 0x69, 0x21, 0x03, 0xcd, 0x09, 0xc3, 0x28,
        0xd6, 0x06, 0xa7, 0x57, 0xbd, 0x48, 0x4b, 0x0f, 0x79, 0x0f, 0xf8, 0x2f, 0xf0, 0x0a, 0x41,
        0x94, 0xd8, 0x8c, 0xa8,
    ];

    const EXPECTED_CDI_ATTEST: &[u8; CDI_SIZE] = &[
        0xfa, 0x3c, 0x2f, 0x58, 0x37, 0xf5, 0x8e, 0x96, 0x16, 0x09, 0xf5, 0x22, 0xa1, 0xf1, 0xba,
        0xaa, 0x19, 0x95, 0x01, 0x79, 0x2e, 0x60, 0x56, 0xaf, 0xf6, 0x41, 0xe7, 0xff, 0x48, 0xf5,
        0x3a, 0x08,
    ];

    const EXPECTED_CDI_PRIVATE_KEY_SEED: &[u8] = &[
        0x5f, 0xcc, 0x8e, 0x1a, 0xd1, 0xc2, 0xb3, 0xe9, 0xfb, 0xe1, 0x68, 0xf0, 0xf6, 0x98, 0xfe,
        0x0d, 0xee, 0xd4, 0xb5, 0x18, 0xcb, 0x59, 0x70, 0x2d, 0xee, 0x06, 0xe5, 0x70, 0xf1, 0x72,
        0x02, 0x6e,
    ];

    const EXPECTED_PUB_KEY: &[u8] = &[
        0x47, 0x42, 0x4b, 0xbd, 0xd7, 0x23, 0xb4, 0xcd, 0xca, 0xe2, 0x8e, 0xdc, 0x6b, 0xfc, 0x23,
        0xc9, 0x21, 0x5c, 0x48, 0x21, 0x47, 0xee, 0x5b, 0xfa, 0xaf, 0x88, 0x9a, 0x52, 0xf1, 0x61,
        0x06, 0x37,
    ];
    const EXPECTED_PRIV_KEY: &[u8] = &[
        0x5f, 0xcc, 0x8e, 0x1a, 0xd1, 0xc2, 0xb3, 0xe9, 0xfb, 0xe1, 0x68, 0xf0, 0xf6, 0x98, 0xfe,
        0x0d, 0xee, 0xd4, 0xb5, 0x18, 0xcb, 0x59, 0x70, 0x2d, 0xee, 0x06, 0xe5, 0x70, 0xf1, 0x72,
        0x02, 0x6e, 0x47, 0x42, 0x4b, 0xbd, 0xd7, 0x23, 0xb4, 0xcd, 0xca, 0xe2, 0x8e, 0xdc, 0x6b,
        0xfc, 0x23, 0xc9, 0x21, 0x5c, 0x48, 0x21, 0x47, 0xee, 0x5b, 0xfa, 0xaf, 0x88, 0x9a, 0x52,
        0xf1, 0x61, 0x06, 0x37,
    ];

    const EXPECTED_SIGNATURE: &[u8] = &[
        0x44, 0xae, 0xcc, 0xe2, 0xb9, 0x96, 0x18, 0x39, 0x0e, 0x61, 0x0f, 0x53, 0x07, 0xbf, 0xf2,
        0x32, 0x3d, 0x44, 0xd4, 0xf2, 0x07, 0x23, 0x30, 0x85, 0x32, 0x18, 0xd2, 0x69, 0xb8, 0x29,
        0x3c, 0x26, 0xe6, 0x0d, 0x9c, 0xa5, 0xc2, 0x73, 0xcd, 0x8c, 0xb8, 0x3c, 0x3e, 0x5b, 0xfd,
        0x62, 0x8d, 0xf6, 0xc4, 0x27, 0xa6, 0xe9, 0x11, 0x06, 0x5a, 0xb2, 0x2b, 0x64, 0xf7, 0xfc,
        0xbb, 0xab, 0x4a, 0x0e,
    ];

    #[test]
    fn hash_derive_sign_verify() {
        let (pub_key, priv_key) = get_test_key_pair();

        let mut signature = sign(b"MyMessage", priv_key.as_array()).unwrap();
        assert_eq!(&signature, EXPECTED_SIGNATURE);
        assert!(verify(b"MyMessage", &signature, &pub_key).is_ok());
        assert!(verify(b"MyMessage_fail", &signature, &pub_key).is_err());
        signature[0] += 1;
        assert!(verify(b"MyMessage", &signature, &pub_key).is_err());
    }

    #[test]
    fn sign_cose_sign1_verify() {
        let (pub_key, priv_key) = get_test_key_pair();

        let signature_res = retry_sign_cose_sign1(b"MyMessage", b"MyAad", priv_key.as_array());
        assert!(signature_res.is_ok());
        let signature = signature_res.unwrap();
        let cose_sign1_res = CoseSign1::from_slice(&signature);
        assert!(cose_sign1_res.is_ok());
        let mut cose_sign1 = cose_sign1_res.unwrap();

        let mut verify_result =
            cose_sign1.verify_signature(b"MyAad", |sign, data| verify(data, sign, &pub_key));
        assert!(verify_result.is_ok());

        verify_result =
            cose_sign1.verify_signature(b"BadAad", |sign, data| verify(data, sign, &pub_key));
        assert!(verify_result.is_err());

        // if we modify the signature, the payload should no longer verify
        cose_sign1.signature.push(0xAA);
        verify_result =
            cose_sign1.verify_signature(b"MyAad", |sign, data| verify(data, sign, &pub_key));
        assert!(verify_result.is_err());
    }

    struct TestArtifactsForSigning {}

    impl DiceArtifacts for TestArtifactsForSigning {
        fn cdi_attest(&self) -> &[u8; CDI_SIZE] {
            EXPECTED_CDI_ATTEST
        }

        fn cdi_seal(&self) -> &[u8; CDI_SIZE] {
            unimplemented!("no test functionality depends on this")
        }

        fn bcc(&self) -> Option<&[u8]> {
            unimplemented!("no test functionality depends on this")
        }
    }

    #[test]
    fn sign_cose_sign1_with_cdi_leaf_priv_verify() {
        let dice = TestArtifactsForSigning {};

        let signature_res = retry_sign_cose_sign1_with_cdi_leaf_priv(b"MyMessage", b"MyAad", &dice);
        assert!(signature_res.is_ok());
        let signature = signature_res.unwrap();
        let cose_sign1_res = CoseSign1::from_slice(&signature);
        assert!(cose_sign1_res.is_ok());
        let mut cose_sign1 = cose_sign1_res.unwrap();

        let mut verify_result = cose_sign1
            .verify_signature(b"MyAad", |sign, data| verify(data, sign, EXPECTED_PUB_KEY));
        assert!(verify_result.is_ok());

        verify_result = cose_sign1
            .verify_signature(b"BadAad", |sign, data| verify(data, sign, EXPECTED_PUB_KEY));
        assert!(verify_result.is_err());

        // if we modify the signature, the payload should no longer verify
        cose_sign1.signature.push(0xAA);
        verify_result = cose_sign1
            .verify_signature(b"MyAad", |sign, data| verify(data, sign, EXPECTED_PUB_KEY));
        assert!(verify_result.is_err());
    }

    fn get_test_key_pair() -> (Vec<u8>, PrivateKey) {
        let seed = hash(b"MySeedString").unwrap();
        assert_eq!(seed, EXPECTED_SEED);
        let cdi_attest = &seed[..CDI_SIZE];
        assert_eq!(cdi_attest, EXPECTED_CDI_ATTEST);
        let cdi_private_key_seed =
            derive_cdi_private_key_seed(cdi_attest.try_into().unwrap()).unwrap();
        assert_eq!(cdi_private_key_seed.as_array(), EXPECTED_CDI_PRIVATE_KEY_SEED);
        let (pub_key, priv_key) = keypair_from_seed(cdi_private_key_seed.as_array()).unwrap();
        assert_eq!(&pub_key, EXPECTED_PUB_KEY);
        assert_eq!(priv_key.as_array(), EXPECTED_PRIV_KEY);

        (pub_key, priv_key)
    }
}
