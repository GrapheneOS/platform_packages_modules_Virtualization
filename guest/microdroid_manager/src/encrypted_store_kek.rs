// Copyright 2025, The Android Open Source Project
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

//! Manages Key Encryption Keys (KEKs) used to set up encrypted store.

use anyhow::{Context, Result};
use keystore2_crypto::ZVec;
use openssl::symm::{decrypt_aead, encrypt_aead, Cipher};

/// Size of the AES256-GCM tag
const AES_256_GCM_TAG_LENGTH: usize = 16;

/// Size of the AES256-GCM nonce
const AES_256_GCM_NONCE_LENGTH: usize = 12;

/// Encrypts key_to_encrypt using provided encryption_key.
pub fn encrypt_kek(key_to_encrypt: &[u8], encryption_key: &[u8]) -> Result<ZVec> {
    let mut result =
        ZVec::new(AES_256_GCM_NONCE_LENGTH + key_to_encrypt.len() + AES_256_GCM_TAG_LENGTH)?;

    let nonce = rand::random::<[u8; AES_256_GCM_NONCE_LENGTH]>();
    let mut tag = [0; AES_256_GCM_TAG_LENGTH];
    let cipher = Cipher::aes_256_gcm();
    let aad = [0; 0];
    let ciphertext =
        encrypt_aead(cipher, encryption_key, Some(&nonce), &aad, key_to_encrypt, &mut tag)?;

    result[0..AES_256_GCM_NONCE_LENGTH].copy_from_slice(&nonce);
    let cipher_start_idx = AES_256_GCM_NONCE_LENGTH;
    let tag_start_idx = cipher_start_idx + ciphertext.len();
    result[cipher_start_idx..tag_start_idx].copy_from_slice(&ciphertext);
    result[tag_start_idx..].copy_from_slice(&tag);
    Ok(result)
}

/// Decrypts encrypted_key using provided encryption_key.
pub fn decrypt_kek(encrypted_key: &[u8], encryption_key: &[u8]) -> Result<ZVec> {
    let cipher_start_idx = AES_256_GCM_NONCE_LENGTH;
    let tag_start_idx = encrypted_key.len() - AES_256_GCM_TAG_LENGTH;

    let nonce = &encrypted_key[0..AES_256_GCM_NONCE_LENGTH];
    let ciphertext = &encrypted_key[cipher_start_idx..tag_start_idx];
    let tag = &encrypted_key[tag_start_idx..];

    let cipher = Cipher::aes_256_gcm();
    let aad = [0; 0];
    let plaintext = decrypt_aead(cipher, encryption_key, Some(nonce), &aad, ciphertext, tag)
        .context("decrypt_aead failed")?;

    ZVec::try_from(plaintext).context("conversion to ZVec failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex::FromHex;

    #[test]
    fn test_encrypt_decrypt_kek() -> Result<()> {
        // Randomly generated, trust me.
        let encryption_key =
            Vec::from_hex("9642c6f9779a29e58e0c7cc36c9b46e8b099cc0284d12b781b9414e2711229d8")?;
        let key_to_encrypt =
            Vec::from_hex("d313972ab3f9345c682035fc49955842940857f78efaa950de676c7d490ed1bf")?;

        let encrypted_kek = encrypt_kek(&key_to_encrypt, &encryption_key)?;
        let decrypted_kek = decrypt_kek(&encrypted_kek, &encryption_key)?;

        assert_eq!(&key_to_encrypt[..], &decrypted_kek[..]);
        Ok(())
    }
}
