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
use bssl_crypto::aead::{Aead, Aes256Gcm};
use keystore2_crypto::ZVec;

/// Encrypts key_to_encrypt using provided encryption_key.
pub fn encrypt_kek(key_to_encrypt: &[u8], encryption_key: &[u8]) -> Result<Vec<u8>> {
    let encryption_key = encryption_key.try_into().context("wrong key size")?;
    let nonce: [u8; 12] = bssl_crypto::rand_array();
    let ciphertext = Aes256Gcm::new(encryption_key).seal(&nonce, key_to_encrypt, &[]);
    Ok([&nonce, ciphertext.as_slice()].concat())
}

/// Decrypts encrypted_key using provided encryption_key.
pub fn decrypt_kek(encrypted_key: &[u8], encryption_key: &[u8]) -> Result<ZVec> {
    let encryption_key = encryption_key.try_into().context("wrong key size")?;
    let (nonce, ciphertext) = encrypted_key.split_first_chunk::<12>().unwrap();
    let plaintext =
        Aes256Gcm::new(encryption_key).open(nonce, ciphertext, &[]).context("could not decrypt")?;
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
