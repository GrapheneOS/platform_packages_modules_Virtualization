/*
 * Copyright (C) 2022 The Android Open Source Project
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

//! Algorithms used for APK Signature Scheme.

use anyhow::{ensure, Context, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use bytes::{Buf, Bytes};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::{FromPrimitive, ToPrimitive};
use openssl::hash::MessageDigest;
use openssl::pkey::{self, PKey};
use openssl::rsa::Padding;
use openssl::sign::Verifier;
use serde::{Deserialize, Serialize};
use std::io::Read;

use crate::bytes_ext::ReadFromBytes;

/// [Signature Algorithm IDs]: https://source.android.com/docs/security/apksigning/v2#signature-algorithm-ids
/// [SignatureAlgorithm.java]: (tools/apksig/src/main/java/com/android/apksig/internal/apk/SignatureAlgorithm.java)
///
/// Some of the algorithms are not implemented. See b/197052981.
#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, Default, Eq, PartialEq, FromPrimitive, ToPrimitive,
)]
#[repr(u32)]
pub enum SignatureAlgorithmID {
    /// RSASSA-PSS with SHA2-256 digest, SHA2-256 MGF1, 32 bytes of salt, trailer: 0xbc, content
    /// digested using SHA2-256 in 1 MB chunks.
    #[default]
    RsaPssWithSha256 = 0x0101,

    /// RSASSA-PSS with SHA2-512 digest, SHA2-512 MGF1, 64 bytes of salt, trailer: 0xbc, content
    /// digested using SHA2-512 in 1 MB chunks.
    RsaPssWithSha512 = 0x0102,

    /// RSASSA-PKCS1-v1_5 with SHA2-256 digest, content digested using SHA2-256 in 1 MB chunks.
    RsaPkcs1V15WithSha256 = 0x0103,

    /// RSASSA-PKCS1-v1_5 with SHA2-512 digest, content digested using SHA2-512 in 1 MB chunks.
    RsaPkcs1V15WithSha512 = 0x0104,

    /// ECDSA with SHA2-256 digest, content digested using SHA2-256 in 1 MB chunks.
    EcdsaWithSha256 = 0x0201,

    /// ECDSA with SHA2-512 digest, content digested using SHA2-512 in 1 MB chunks.
    EcdsaWithSha512 = 0x0202,

    /// DSA with SHA2-256 digest, content digested using SHA2-256 in 1 MB chunks.
    /// Signing is done deterministically according to RFC 6979.
    DsaWithSha256 = 0x0301,

    /// RSASSA-PKCS1-v1_5 with SHA2-256 digest, content digested using SHA2-256 in 4 KB
    /// chunks, in the same way fsverity operates. This digest and the content length
    /// (before digestion, 8 bytes in little endian) construct the final digest.
    VerityRsaPkcs1V15WithSha256 = 0x0421,

    /// ECDSA with SHA2-256 digest, content digested using SHA2-256 in 4 KB chunks, in the
    /// same way fsverity operates. This digest and the content length (before digestion,
    /// 8 bytes in little endian) construct the final digest.
    VerityEcdsaWithSha256 = 0x0423,

    /// DSA with SHA2-256 digest, content digested using SHA2-256 in 4 KB chunks, in the
    /// same way fsverity operates. This digest and the content length (before digestion,
    /// 8 bytes in little endian) construct the final digest.
    VerityDsaWithSha256 = 0x0425,
}

impl ReadFromBytes for Option<SignatureAlgorithmID> {
    fn read_from_bytes(buf: &mut Bytes) -> Result<Self> {
        Ok(SignatureAlgorithmID::from_u32(buf.get_u32_le()))
    }
}

impl SignatureAlgorithmID {
    /// Converts the signature algorithm ID to the corresponding u32.
    pub fn to_u32(&self) -> u32 {
        ToPrimitive::to_u32(self).expect("Unsupported algorithm for to_u32.")
    }

    pub(crate) fn new_verifier<'a>(
        &self,
        public_key: &'a PKey<pkey::Public>,
    ) -> Result<Verifier<'a>> {
        ensure!(
            !matches!(
                self,
                SignatureAlgorithmID::DsaWithSha256 | SignatureAlgorithmID::VerityDsaWithSha256
            ),
            "Algorithm '{:?}' is not supported in openssl to build this verifier (b/197052981).",
            self
        );
        ensure!(public_key.id() == self.pkey_id(), "Public key has the wrong ID");
        let mut verifier = Verifier::new(self.new_message_digest(), public_key)?;
        if public_key.id() == pkey::Id::RSA {
            verifier.set_rsa_padding(self.rsa_padding())?;
        }
        Ok(verifier)
    }

    /// Returns the message digest corresponding to the signature algorithm
    /// according to the spec [Signature Algorithm IDs].
    pub(crate) fn new_message_digest(&self) -> MessageDigest {
        match self {
            SignatureAlgorithmID::RsaPssWithSha256
            | SignatureAlgorithmID::RsaPkcs1V15WithSha256
            | SignatureAlgorithmID::EcdsaWithSha256
            | SignatureAlgorithmID::DsaWithSha256
            | SignatureAlgorithmID::VerityRsaPkcs1V15WithSha256
            | SignatureAlgorithmID::VerityEcdsaWithSha256
            | SignatureAlgorithmID::VerityDsaWithSha256 => MessageDigest::sha256(),
            SignatureAlgorithmID::RsaPssWithSha512
            | SignatureAlgorithmID::RsaPkcs1V15WithSha512
            | SignatureAlgorithmID::EcdsaWithSha512 => MessageDigest::sha512(),
        }
    }

    /// DSA is not directly supported in openssl today. See b/197052981.
    pub(crate) fn is_supported(&self) -> bool {
        !matches!(
            self,
            SignatureAlgorithmID::DsaWithSha256 | SignatureAlgorithmID::VerityDsaWithSha256,
        )
    }

    fn pkey_id(&self) -> pkey::Id {
        match self {
            SignatureAlgorithmID::RsaPssWithSha256
            | SignatureAlgorithmID::RsaPssWithSha512
            | SignatureAlgorithmID::RsaPkcs1V15WithSha256
            | SignatureAlgorithmID::RsaPkcs1V15WithSha512
            | SignatureAlgorithmID::VerityRsaPkcs1V15WithSha256 => pkey::Id::RSA,
            SignatureAlgorithmID::EcdsaWithSha256
            | SignatureAlgorithmID::EcdsaWithSha512
            | SignatureAlgorithmID::VerityEcdsaWithSha256 => pkey::Id::EC,
            SignatureAlgorithmID::DsaWithSha256 | SignatureAlgorithmID::VerityDsaWithSha256 => {
                pkey::Id::DSA
            }
        }
    }

    fn rsa_padding(&self) -> Padding {
        match self {
            SignatureAlgorithmID::RsaPssWithSha256 | SignatureAlgorithmID::RsaPssWithSha512 => {
                Padding::PKCS1_PSS
            }
            SignatureAlgorithmID::RsaPkcs1V15WithSha256
            | SignatureAlgorithmID::VerityRsaPkcs1V15WithSha256
            | SignatureAlgorithmID::RsaPkcs1V15WithSha512 => Padding::PKCS1,
            SignatureAlgorithmID::EcdsaWithSha256
            | SignatureAlgorithmID::EcdsaWithSha512
            | SignatureAlgorithmID::VerityEcdsaWithSha256
            | SignatureAlgorithmID::DsaWithSha256
            | SignatureAlgorithmID::VerityDsaWithSha256 => Padding::NONE,
        }
    }

    pub(crate) fn content_digest_algorithm(&self) -> ContentDigestAlgorithm {
        match self {
            SignatureAlgorithmID::RsaPssWithSha256
            | SignatureAlgorithmID::RsaPkcs1V15WithSha256
            | SignatureAlgorithmID::EcdsaWithSha256
            | SignatureAlgorithmID::DsaWithSha256 => ContentDigestAlgorithm::ChunkedSha256,
            SignatureAlgorithmID::RsaPssWithSha512
            | SignatureAlgorithmID::RsaPkcs1V15WithSha512
            | SignatureAlgorithmID::EcdsaWithSha512 => ContentDigestAlgorithm::ChunkedSha512,
            SignatureAlgorithmID::VerityRsaPkcs1V15WithSha256
            | SignatureAlgorithmID::VerityEcdsaWithSha256
            | SignatureAlgorithmID::VerityDsaWithSha256 => {
                ContentDigestAlgorithm::VerityChunkedSha256
            }
        }
    }
}

/// The rank of the content digest algorithm in this enum is used to help pick
/// v4 apk digest.
/// According to APK Signature Scheme v4, [apk digest] is the first available
/// content digest of the highest rank (rank N).
///
/// This rank was also used for step 3a of the v3 signature verification.
///
/// [apk digest]: https://source.android.com/docs/security/features/apksigning/v4#apk-digest
/// [v3 verification]: https://source.android.com/docs/security/apksigning/v3#v3-verification
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ContentDigestAlgorithm {
    ChunkedSha256 = 1,
    VerityChunkedSha256,
    ChunkedSha512,
}

/// Hash algorithms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, FromPrimitive, ToPrimitive, Default)]
#[repr(u32)]
pub enum HashAlgorithm {
    #[default]
    /// SHA-256
    SHA256 = 1,
}

impl HashAlgorithm {
    pub(crate) fn from_read<R: Read>(read: &mut R) -> Result<Self> {
        let val = read.read_u32::<LittleEndian>()?;
        Self::from_u32(val).context(format!("Unsupported hash algorithm: {val}"))
    }
}
