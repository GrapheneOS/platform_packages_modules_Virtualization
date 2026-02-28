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
    use coset::{CborSerializable, CoseSign1};
    #[cfg(feature = "multialg")]
    use diced_open_dice::KeyAlgorithm;
    use diced_open_dice::{
        derive_cdi_certificate_id, derive_cdi_private_key_seed, hash, kdf, keypair_from_seed,
        retry_sign_cose_sign1, retry_sign_cose_sign1_with_cdi_leaf_priv, sign, verify,
        DiceArtifacts, DiceContext, DiceError, PrivateKey, CDI_SIZE, HASH_SIZE, ID_SIZE,
        PRIVATE_KEY_SEED_SIZE,
    };

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

    // hash(b"MySeedString")
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
    const EXPECTED_PRIV_KEY: &[u8; 64] = &[
        0x5f, 0xcc, 0x8e, 0x1a, 0xd1, 0xc2, 0xb3, 0xe9, 0xfb, 0xe1, 0x68, 0xf0, 0xf6, 0x98, 0xfe,
        0x0d, 0xee, 0xd4, 0xb5, 0x18, 0xcb, 0x59, 0x70, 0x2d, 0xee, 0x06, 0xe5, 0x70, 0xf1, 0x72,
        0x02, 0x6e, 0x47, 0x42, 0x4b, 0xbd, 0xd7, 0x23, 0xb4, 0xcd, 0xca, 0xe2, 0x8e, 0xdc, 0x6b,
        0xfc, 0x23, 0xc9, 0x21, 0x5c, 0x48, 0x21, 0x47, 0xee, 0x5b, 0xfa, 0xaf, 0x88, 0x9a, 0x52,
        0xf1, 0x61, 0x06, 0x37,
    ];
    #[cfg(feature = "multialg")]
    const EXPECTED_EC_P256_PUB_KEY: &[u8] = &[
        0xa7, 0x93, 0x70, 0x16, 0xff, 0xe8, 0x3c, 0x23, 0x5f, 0x6b, 0xf9, 0x38, 0x7e, 0x9c, 0xe5,
        0x21, 0xb5, 0x8a, 0x9b, 0x68, 0x5a, 0x2f, 0x62, 0xf4, 0x15, 0x94, 0x1c, 0x61, 0xb3, 0xbb,
        0xe1, 0x26, 0x61, 0x47, 0x97, 0xbf, 0x3a, 0x1f, 0x6b, 0x87, 0x86, 0x47, 0x5e, 0xc3, 0xa6,
        0x8b, 0x95, 0x89, 0x9e, 0x29, 0xd5, 0x55, 0x2a, 0xdd, 0x2a, 0x3f, 0xe5, 0xf0, 0x7a, 0xd6,
        0xc4, 0x7b, 0x64, 0xe0,
    ];
    #[cfg(feature = "multialg")]
    const EXPECTED_EC_P256_PRIV_KEY: &[u8; 64] = &[
        0x62, 0x32, 0x1b, 0xb, 0x5c, 0xac, 0x8f, 0x20, 0x61, 0xb7, 0xa3, 0xbb, 0x46, 0x2b, 0x4e,
        0xb3, 0x3f, 0xa7, 0xf6, 0x9b, 0x2f, 0x5b, 0x80, 0xa8, 0x55, 0x5e, 0x80, 0x26, 0xbb, 0x72,
        0xbe, 0xe7, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
        0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    ];
    const EXPECTED_SIGNATURE: &[u8] = &[
        0x44, 0xae, 0xcc, 0xe2, 0xb9, 0x96, 0x18, 0x39, 0x0e, 0x61, 0x0f, 0x53, 0x07, 0xbf, 0xf2,
        0x32, 0x3d, 0x44, 0xd4, 0xf2, 0x07, 0x23, 0x30, 0x85, 0x32, 0x18, 0xd2, 0x69, 0xb8, 0x29,
        0x3c, 0x26, 0xe6, 0x0d, 0x9c, 0xa5, 0xc2, 0x73, 0xcd, 0x8c, 0xb8, 0x3c, 0x3e, 0x5b, 0xfd,
        0x62, 0x8d, 0xf6, 0xc4, 0x27, 0xa6, 0xe9, 0x11, 0x06, 0x5a, 0xb2, 0x2b, 0x64, 0xf7, 0xfc,
        0xbb, 0xab, 0x4a, 0x0e,
    ];

    #[test]
    fn derive_cdi_private_key_seed_verify() {
        let (pub_key, priv_key) = derive_key_pair(None);

        assert_eq!(&pub_key, EXPECTED_PUB_KEY);
        assert_eq!(priv_key.as_array(), EXPECTED_PRIV_KEY);
    }

    #[cfg(feature = "multialg")]
    #[test]
    fn derive_cdi_private_key_seed_verify_multialg() {
        let (pub_key, priv_key) = derive_key_pair(Some(DiceContext {
            authority_algorithm: KeyAlgorithm::EcdsaP256,
            subject_algorithm: KeyAlgorithm::EcdsaP256,
        }));

        assert_eq!(&pub_key, EXPECTED_EC_P256_PUB_KEY);
        assert_eq!(priv_key.as_array(), EXPECTED_EC_P256_PRIV_KEY);
    }

    #[test]
    fn hash_derive_sign_verify() {
        let mut signature =
            sign(b"MyMessage", EXPECTED_PRIV_KEY).expect("Couldn't generate signature");
        assert_eq!(&signature, EXPECTED_SIGNATURE);

        assert!(verify(None, b"MyMessage", &signature, EXPECTED_PUB_KEY).is_ok());

        assert!(verify(None, b"MyMessage_fail", &signature, EXPECTED_PUB_KEY).is_err());

        signature[0] += 1;
        assert!(verify(None, b"MyMessage", &signature, EXPECTED_PUB_KEY).is_err());
    }

    #[test]
    fn sign_cose_sign1_verify() {
        build_cose_sign1_and_verify(
            retry_sign_cose_sign1,
            None,
            b"MyMessage",
            b"MyAad",
            EXPECTED_PRIV_KEY,
            EXPECTED_PUB_KEY,
        );
    }

    #[cfg(feature = "multialg")]
    #[test]
    fn sign_cose_sign1_verify_multialg() {
        let dice_context = DiceContext {
            authority_algorithm: KeyAlgorithm::EcdsaP256,
            subject_algorithm: KeyAlgorithm::EcdsaP256,
        };

        build_cose_sign1_and_verify(
            retry_sign_cose_sign1,
            Some(dice_context),
            b"MyMessage",
            b"MyAad",
            EXPECTED_EC_P256_PRIV_KEY,
            EXPECTED_EC_P256_PUB_KEY,
        );
    }

    struct TestArtifactsForSigning;

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
    fn sign_cose_sign1_with_cdi_leaf_priv_large_msg() {
        build_cose_sign1_and_verify(
            retry_sign_cose_sign1_with_cdi_leaf_priv,
            None,
            &vec![0x60; 16 * 1024],
            b"MyAad",
            &TestArtifactsForSigning,
            EXPECTED_PUB_KEY,
        );
    }

    #[test]
    fn sign_cose_sign1_with_cdi_leaf_priv_large_aad() {
        build_cose_sign1_and_verify(
            retry_sign_cose_sign1_with_cdi_leaf_priv,
            None,
            b"MyMessage",
            &vec![0x7a; 255],
            &TestArtifactsForSigning,
            EXPECTED_PUB_KEY,
        );
    }

    #[test]
    fn sign_cose_sign1_with_cdi_leaf_priv_empty_msg() {
        build_cose_sign1_and_verify(
            retry_sign_cose_sign1_with_cdi_leaf_priv,
            None,
            b"",
            b"MyAad",
            &TestArtifactsForSigning,
            EXPECTED_PUB_KEY,
        );
    }

    #[test]
    fn sign_cose_sign1_with_cdi_leaf_priv_empty_aad() {
        build_cose_sign1_and_verify(
            retry_sign_cose_sign1_with_cdi_leaf_priv,
            None,
            b"MyMessage",
            b"",
            &TestArtifactsForSigning,
            EXPECTED_PUB_KEY,
        );
    }

    #[test]
    fn sign_cose_sign1_with_cdi_leaf_priv_verify() {
        build_cose_sign1_and_verify(
            retry_sign_cose_sign1_with_cdi_leaf_priv,
            None,
            b"MyMessage",
            b"MyAad",
            &TestArtifactsForSigning,
            EXPECTED_PUB_KEY,
        );
    }

    #[cfg(feature = "multialg")]
    #[test]
    fn sign_cose_sign1_with_cdi_leaf_priv_verify_multialg() {
        let dice_context = DiceContext {
            authority_algorithm: KeyAlgorithm::EcdsaP256,
            subject_algorithm: KeyAlgorithm::EcdsaP256,
        };

        build_cose_sign1_and_verify(
            retry_sign_cose_sign1_with_cdi_leaf_priv,
            Some(dice_context),
            b"MyMessage",
            b"MyAad",
            &TestArtifactsForSigning,
            EXPECTED_EC_P256_PUB_KEY,
        );
    }

    fn derive_key_pair(dice_context: Option<DiceContext>) -> (Vec<u8>, PrivateKey) {
        let seed = hash(b"MySeedString").expect("Couldn't create the seed");
        assert_eq!(seed, EXPECTED_SEED);

        let cdi_attest = &seed[..CDI_SIZE];
        assert_eq!(cdi_attest, EXPECTED_CDI_ATTEST);

        let cdi_private_key_seed = derive_cdi_private_key_seed(cdi_attest.try_into().unwrap())
            .expect("Couldn't derive cdi private key seed");
        assert_eq!(cdi_private_key_seed.as_array(), EXPECTED_CDI_PRIVATE_KEY_SEED);

        keypair_from_seed(dice_context, cdi_private_key_seed.as_array())
            .expect("Couldn't derive key pair")
    }

    fn build_cose_sign1_and_verify<F, T>(
        builder: F,
        dice_context: Option<DiceContext>,
        message: &[u8],
        aad: &[u8],
        key: T,
        pub_key: &[u8],
    ) where
        F: FnOnce(Option<DiceContext>, &[u8], &[u8], T) -> Result<Vec<u8>, DiceError>,
    {
        let signature =
            builder(dice_context, message, aad, key).expect("Couldn't create the signature");
        let mut cose_sign1 = CoseSign1::from_slice(&signature).expect("Couldn't create CoseSign1");
        let verifier = |cose_sign1: &CoseSign1, aad| {
            cose_sign1.verify_signature(aad, |sign, data| verify(dice_context, data, sign, pub_key))
        };

        assert!(verifier(&cose_sign1, aad).is_ok());

        let mut bad_aad = Vec::from(aad);
        bad_aad.push(0xAA);
        assert!(verifier(&cose_sign1, &bad_aad).is_err());

        // if we modify the signature, the payload should no longer verify
        cose_sign1.signature.push(0xAA);
        assert!(verifier(&cose_sign1, aad).is_err());
    }
}
