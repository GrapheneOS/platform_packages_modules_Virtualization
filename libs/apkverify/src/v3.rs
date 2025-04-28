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

//! Verifies APK Signature Scheme V3
//!
//! [v3 verification]: https://source.android.com/security/apksigning/v3#verification

use anyhow::{ensure, Context, Result};
use bytes::Bytes;
use openssl::pkey::{self, PKey};
use openssl::x509::X509;
use std::fs::File;
use std::io::{Read, Seek};
use std::ops::RangeInclusive;
use std::path::Path;

use crate::algorithms::SignatureAlgorithmID;
use crate::bytes_ext::{BytesExt, LengthPrefixed, ReadFromBytes};
use crate::sigutil::ApkSections;

pub const APK_SIGNATURE_SCHEME_V3_BLOCK_ID: u32 = 0xf05368c0;

type Signers = LengthPrefixed<Vec<LengthPrefixed<Signer>>>;

#[derive(Debug)]
pub(crate) struct Signer {
    signed_data: LengthPrefixed<Bytes>, // not verified yet
    min_sdk: u32,
    max_sdk: u32,
    signatures: LengthPrefixed<Vec<LengthPrefixed<Signature>>>,
    public_key: PKey<pkey::Public>,
}

/// Contains the signed data part of an APK v3 signature.
#[derive(Debug)]
pub struct SignedData {
    digests: LengthPrefixed<Vec<LengthPrefixed<Digest>>>,
    certificates: LengthPrefixed<Vec<LengthPrefixed<X509Certificate>>>,
    min_sdk: u32,
    max_sdk: u32,
    #[allow(dead_code)]
    additional_attributes: LengthPrefixed<Vec<LengthPrefixed<AdditionalAttributes>>>,
}

#[derive(Debug)]
pub(crate) struct Signature {
    /// Option is used here to allow us to ignore unsupported algorithm.
    pub(crate) signature_algorithm_id: Option<SignatureAlgorithmID>,
    signature: LengthPrefixed<Bytes>,
}

#[derive(Debug)]
struct Digest {
    signature_algorithm_id: Option<SignatureAlgorithmID>,
    digest: LengthPrefixed<Bytes>,
}

type X509Certificate = Bytes;
type AdditionalAttributes = Bytes;

/// Verifies APK Signature Scheme v3 signatures of the provided APK and returns the SignedData from
/// the signature.
pub fn verify<P: AsRef<Path>>(apk_path: P, current_sdk: u32) -> Result<SignedData> {
    let apk = File::open(apk_path.as_ref())?;
    let (signer, mut sections) = extract_signer_and_apk_sections(apk, current_sdk)?;
    signer.verify(&mut sections)
}

/// Extracts the SignedData from the signature of the given APK. (The signature is not verified.)
pub fn extract_signed_data<P: AsRef<Path>>(apk_path: P, current_sdk: u32) -> Result<SignedData> {
    let apk = File::open(apk_path.as_ref())?;
    let (signer, _) = extract_signer_and_apk_sections(apk, current_sdk)?;
    signer.parse_signed_data()
}

pub(crate) fn extract_signer_and_apk_sections<R: Read + Seek>(
    apk: R,
    current_sdk: u32,
) -> Result<(Signer, ApkSections<R>)> {
    let mut sections = ApkSections::new(apk)?;
    let mut block = sections.find_signature(APK_SIGNATURE_SCHEME_V3_BLOCK_ID).context(
        "Fallback to v2 when v3 block not found is not yet implemented.", // b/197052981
    )?;
    let signers = block.read::<Signers>()?.into_inner();
    let mut supported =
        signers.into_iter().filter(|s| s.sdk_range().contains(&current_sdk)).collect::<Vec<_>>();
    ensure!(
        supported.len() == 1,
        "APK Signature Scheme V3 only supports one signer: {} signers found.",
        supported.len()
    );
    Ok((supported.pop().unwrap().into_inner(), sections))
}

impl Signer {
    fn sdk_range(&self) -> RangeInclusive<u32> {
        self.min_sdk..=self.max_sdk
    }

    /// Selects the signature that has the strongest supported `SignatureAlgorithmID`.
    /// The strongest signature is used in both v3 verification and v4 apk digest computation.
    pub(crate) fn strongest_signature(&self) -> Result<&Signature> {
        Ok(self
            .signatures
            .iter()
            .filter(|sig| sig.signature_algorithm_id.is_some_and(|algo| algo.is_supported()))
            .max_by_key(|sig| sig.signature_algorithm_id.unwrap().content_digest_algorithm())
            .context("No supported APK signatures found; DSA is not supported")?)
    }

    pub(crate) fn find_digest_by_algorithm(
        &self,
        algorithm_id: SignatureAlgorithmID,
    ) -> Result<Box<[u8]>> {
        let signed_data: SignedData = self.signed_data.slice(..).read()?;
        let digest = signed_data.find_digest_by_algorithm(algorithm_id)?;
        Ok(digest.digest.as_ref().to_vec().into_boxed_slice())
    }

    /// Verifies a signature over the signed data using the public key.
    fn verify_signature(&self, signature: &Signature) -> Result<()> {
        let mut verifier = signature
            .signature_algorithm_id
            .context("Unsupported algorithm")?
            .new_verifier(&self.public_key)?;
        verifier.update(&self.signed_data)?;
        ensure!(verifier.verify(&signature.signature)?, "Signature is invalid.");
        Ok(())
    }

    /// Returns the signed data, converted from bytes.
    fn parse_signed_data(&self) -> Result<SignedData> {
        self.signed_data.slice(..).read()
    }

    /// The steps in this method implements APK Signature Scheme v3 verification step 3.
    fn verify<R: Read + Seek>(&self, sections: &mut ApkSections<R>) -> Result<SignedData> {
        // 1. Choose the strongest supported signature algorithm ID from signatures.
        let strongest = self.strongest_signature()?;

        // 2. Verify the corresponding signature from signatures against signed data using public
        // key.
        self.verify_signature(strongest)?;

        // It is now safe to parse signed data.
        let verified_signed_data = self.parse_signed_data()?;

        // 3. Verify the min and max SDK versions in the signed data match those specified for the
        //    signer.
        ensure!(
            self.sdk_range() == verified_signed_data.sdk_range(),
            "SDK versions mismatch between signed and unsigned in v3 signer block."
        );

        // 4. Verify that the ordered list of signature algorithm IDs in digests and signatures is
        //    identical. (This is to prevent signature stripping/addition.)
        ensure!(
            self.signatures
                .iter()
                .map(|sig| sig.signature_algorithm_id)
                .eq(verified_signed_data.digests.iter().map(|dig| dig.signature_algorithm_id)),
            "Signature algorithms don't match between digests and signatures records"
        );

        // 5. Compute the digest of APK contents using the same digest algorithm as the digest
        //    algorithm used by the signature algorithm.
        let digest = verified_signed_data.find_digest_by_algorithm(
            strongest.signature_algorithm_id.context("Unsupported algorithm")?,
        )?;
        let computed = sections.compute_digest(digest.signature_algorithm_id.unwrap())?;

        // 6. Verify that the computed digest is identical to the corresponding digest from digests.
        ensure!(
            computed == digest.digest.as_ref(),
            "Digest mismatch: computed={:?} vs expected={:?}",
            hex::encode(&computed),
            hex::encode(digest.digest.as_ref()),
        );

        // 7. Verify that public key of the first certificate of certificates is identical to public
        //    key.
        let cert = X509::from_der(verified_signed_data.first_certificate_der()?)?;
        ensure!(
            cert.public_key()?.public_eq(&self.public_key),
            "Public key mismatch between certificate and signature record"
        );

        // TODO(b/245914104)
        // 8. If the proof-of-rotation attribute exists for the signer verify that the
        // struct is valid and this signer is the last certificate in the list.

        Ok(verified_signed_data)
    }
}

impl SignedData {
    /// Returns the first X.509 certificate in the signed data, encoded in DER form. (All other
    /// certificates are ignored for v3; this certificate describes the public key that was actually
    /// used to sign the APK.)
    pub fn first_certificate_der(&self) -> Result<&[u8]> {
        Ok(self.certificates.first().context("No certificates listed")?)
    }

    fn sdk_range(&self) -> RangeInclusive<u32> {
        self.min_sdk..=self.max_sdk
    }

    fn find_digest_by_algorithm(&self, algorithm_id: SignatureAlgorithmID) -> Result<&Digest> {
        Ok(self
            .digests
            .iter()
            .find(|&dig| dig.signature_algorithm_id == Some(algorithm_id))
            .context(format!("Digest not found for algorithm: {:?}", algorithm_id))?)
    }
}

// ReadFromBytes implementations
// TODO(b/190343842): add derive macro: #[derive(ReadFromBytes)]

impl ReadFromBytes for Signer {
    fn read_from_bytes(buf: &mut Bytes) -> Result<Self> {
        Ok(Self {
            signed_data: buf.read()?,
            min_sdk: buf.read()?,
            max_sdk: buf.read()?,
            signatures: buf.read()?,
            public_key: buf.read()?,
        })
    }
}

impl ReadFromBytes for SignedData {
    fn read_from_bytes(buf: &mut Bytes) -> Result<Self> {
        Ok(Self {
            digests: buf.read()?,
            certificates: buf.read()?,
            min_sdk: buf.read()?,
            max_sdk: buf.read()?,
            additional_attributes: buf.read()?,
        })
    }
}

impl ReadFromBytes for Signature {
    fn read_from_bytes(buf: &mut Bytes) -> Result<Self> {
        Ok(Signature { signature_algorithm_id: buf.read()?, signature: buf.read()? })
    }
}

impl ReadFromBytes for Digest {
    fn read_from_bytes(buf: &mut Bytes) -> Result<Self> {
        Ok(Self { signature_algorithm_id: buf.read()?, digest: buf.read()? })
    }
}

impl ReadFromBytes for PKey<pkey::Public> {
    fn read_from_bytes(buf: &mut Bytes) -> Result<Self> {
        let raw_public_key = buf.read::<LengthPrefixed<Bytes>>()?;
        Ok(PKey::public_key_from_der(raw_public_key.as_ref())?)
    }
}
