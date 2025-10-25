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

//! Handles the encryption and decryption of the key blob.

use crate::ops::KeyDerivationOps;
use alloc::vec::Vec;
use bssl_crypto::aead::{Aead, Aes256Gcm};
use bssl_crypto::hkdf::{self, HkdfSha512};
use serde::{Deserialize, Serialize};
use service_vm_comm::{RequestProcessingError, Result};
use zeroize::Zeroizing;

/// The KEK (Key Encryption Key) info is used as information to derive the KEK using HKDF.
const KEK_INFO: &[u8] = b"rialto keyblob kek";

/// An all-zero nonce is utilized to encrypt the private key. This is because each key
/// undergoes encryption using a distinct KEK, which is derived from a secret and a random
/// salt. Since the uniqueness of the IV/key combination is already guaranteed by the uniqueness
/// of the KEK, there is no need for an additional random nonce.
const PRIVATE_KEY_NONCE: &[u8; 12] = &[0; 12];

/// Since Service VM functions as both the sender and receiver of the message, no additional data
/// is needed.
const PRIVATE_KEY_AD: &[u8] = &[];

/// A simple implementation of [`KeyDerivationOps`] that holds a raw secret in memory.
pub struct InMemoryKeyDerivationOps {
    secret: Zeroizing<Vec<u8>>,
}

impl From<Zeroizing<Vec<u8>>> for InMemoryKeyDerivationOps {
    fn from(secret: Zeroizing<Vec<u8>>) -> Self {
        Self { secret }
    }
}

impl KeyDerivationOps for InMemoryKeyDerivationOps {
    fn derive_kek(&self, salt: &[u8], info: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        Ok(HkdfSha512::derive(&self.secret, hkdf::Salt::NonEmpty(salt), info).into())
    }
}

// Encrypted key blob.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum EncryptedKeyBlob {
    /// Version 1 key blob.
    V1(EncryptedKeyBlobV1),
}

/// Encrypted key blob version 1.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct EncryptedKeyBlobV1 {
    /// Salt used to derive the KEK.
    kek_salt: [u8; 32],

    /// Private key encrypted with AES-256-GCM.
    encrypted_private_key: Vec<u8>,
}

impl EncryptedKeyBlob {
    pub(crate) fn new(private_key: &[u8], ops: &impl KeyDerivationOps) -> Result<Self> {
        EncryptedKeyBlobV1::new(private_key, ops).map(Self::V1)
    }

    pub(crate) fn decrypt_private_key(
        &self,
        ops: &impl KeyDerivationOps,
    ) -> Result<Zeroizing<Vec<u8>>> {
        match self {
            Self::V1(blob) => blob.decrypt_private_key(ops),
        }
    }
}

impl EncryptedKeyBlobV1 {
    fn new(private_key: &[u8], ops: &impl KeyDerivationOps) -> Result<Self> {
        let kek_salt: [u8; 32] = bssl_crypto::rand_array();
        let kek = ops.derive_kek(&kek_salt, KEK_INFO)?;
        let ciphertext = Aes256Gcm::new(&kek).seal(PRIVATE_KEY_NONCE, private_key, PRIVATE_KEY_AD);
        Ok(Self { kek_salt, encrypted_private_key: ciphertext })
    }

    fn decrypt_private_key(&self, ops: &impl KeyDerivationOps) -> Result<Zeroizing<Vec<u8>>> {
        let kek = ops.derive_kek(&self.kek_salt, KEK_INFO)?;
        let private_key = Aes256Gcm::new(&kek)
            .open(PRIVATE_KEY_NONCE, &self.encrypted_private_key, PRIVATE_KEY_AD)
            .ok_or(RequestProcessingError::FailedToDecryptKeyBlob)?;
        Ok(Zeroizing::new(private_key))
    }
}

pub(crate) fn decrypt_private_key(
    encrypted_key_blob: &[u8],
    ops: &impl KeyDerivationOps,
) -> Result<Zeroizing<Vec<u8>>> {
    let key_blob: EncryptedKeyBlob = cbor_util::deserialize(encrypted_key_blob)?;
    let private_key = key_blob.decrypt_private_key(ops)?;
    Ok(private_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use service_vm_comm::RequestProcessingError;

    /// The test data are generated randomly with /dev/urandom.
    const TEST_KEY: [u8; 32] = [
        0x76, 0xf7, 0xd5, 0x36, 0x1f, 0x78, 0x58, 0x2e, 0x55, 0x2f, 0x88, 0x9d, 0xa3, 0x3e, 0xba,
        0xfb, 0xc1, 0x2b, 0x17, 0x85, 0x24, 0xdc, 0x0e, 0xc4, 0xbf, 0x6d, 0x2e, 0xe8, 0xa8, 0x36,
        0x93, 0x62,
    ];
    const TEST_SECRET1: [u8; 32] = [
        0xac, 0xb1, 0x6b, 0xdf, 0x45, 0x30, 0x20, 0xa5, 0x60, 0x6d, 0x81, 0x07, 0x30, 0x68, 0x6e,
        0x01, 0x3d, 0x5e, 0x86, 0xd6, 0xc6, 0x17, 0xfa, 0xd6, 0xe0, 0xff, 0xd4, 0xf0, 0xb0, 0x7c,
        0x5c, 0x8f,
    ];
    const TEST_SECRET2: [u8; 32] = [
        0x04, 0x6e, 0xca, 0x30, 0x5e, 0x6c, 0x8f, 0xe5, 0x1a, 0x47, 0x12, 0xbc, 0x45, 0xd7, 0xa8,
        0x38, 0xfb, 0x06, 0xc6, 0x44, 0xa1, 0x21, 0x40, 0x0b, 0x48, 0x88, 0xe2, 0x31, 0x64, 0x42,
        0x9d, 0x1c,
    ];

    #[test]
    fn decrypting_keyblob_succeeds_with_the_same_kek() -> Result<()> {
        let ops = InMemoryKeyDerivationOps::from(Zeroizing::new(TEST_SECRET1.to_vec()));
        let encrypted_key_blob = cbor_util::serialize(&EncryptedKeyBlob::new(&TEST_KEY, &ops)?)?;
        let decrypted_key = decrypt_private_key(&encrypted_key_blob, &ops)?;

        assert_eq!(TEST_KEY, decrypted_key.as_slice());
        Ok(())
    }

    #[test]
    fn decrypting_keyblob_fails_with_a_different_kek() -> Result<()> {
        let ops1 = InMemoryKeyDerivationOps::from(Zeroizing::new(TEST_SECRET1.to_vec()));
        let ops2 = InMemoryKeyDerivationOps::from(Zeroizing::new(TEST_SECRET2.to_vec()));
        let encrypted_key_blob = cbor_util::serialize(&EncryptedKeyBlob::new(&TEST_KEY, &ops1)?)?;
        let err = decrypt_private_key(&encrypted_key_blob, &ops2).unwrap_err();

        assert_eq!(RequestProcessingError::FailedToDecryptKeyBlob, err);
        Ok(())
    }
}
