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

//! API for APK Signature Scheme [v4].
//!
//! [v4]: https://source.android.com/security/apksigning/v4

use anyhow::{anyhow, bail, ensure, Context, Result};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::{FromPrimitive, ToPrimitive};
use std::fs;
use std::io::{copy, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::algorithms::{HashAlgorithm, SignatureAlgorithmID};
use crate::hashtree::*;
use crate::v3::extract_signer_and_apk_sections;

/// Gets the v4 [apk_digest]. If `verify` is true, we verify that digest computed
/// with the extracted algorithm is equal to the digest extracted directly from apk.
/// Otherwise, the extracted digest will be returned directly.
///
/// [apk_digest]: https://source.android.com/docs/security/apksigning/v4#apk-digest
pub fn get_apk_digest<R: Read + Seek>(
    apk: R,
    current_sdk: u32,
    verify: bool,
) -> Result<(SignatureAlgorithmID, Box<[u8]>)> {
    let (signer, mut sections) = extract_signer_and_apk_sections(apk, current_sdk)?;
    let strongest_algorithm_id = signer
        .strongest_signature()?
        .signature_algorithm_id
        .context("Strongest signature should contain a valid signature algorithm.")?;
    let extracted_digest = signer.find_digest_by_algorithm(strongest_algorithm_id)?;
    if verify {
        let computed_digest = sections.compute_digest(strongest_algorithm_id)?;
        ensure!(
            computed_digest == extracted_digest.as_ref(),
            "Computed digest does not match the extracted digest."
        );
    }
    Ok((strongest_algorithm_id, extracted_digest))
}

/// `V4Signature` provides access to the various fields in an idsig file.
#[derive(Default)]
pub struct V4Signature<R: Read + Seek> {
    /// Version of the header. Should be 2.
    pub version: Version,
    /// Provides access to the information about how the APK is hashed.
    pub hashing_info: HashingInfo,
    /// Provides access to the information that can be used to verify this file
    pub signing_info: SigningInfo,
    /// Total size of the merkle tree
    pub merkle_tree_size: u32,
    /// Offset of the merkle tree in the idsig file
    pub merkle_tree_offset: u64,

    // Provides access to the underlying data
    data: R,
}

/// `HashingInfo` provides information about how the APK is hashed.
#[derive(Default)]
pub struct HashingInfo {
    /// Hash algorithm used when creating the merkle tree for the APK.
    pub hash_algorithm: HashAlgorithm,
    /// The log size of a block used when creating the merkle tree. 12 if 4k block was used.
    pub log2_blocksize: u8,
    /// The salt used when creating the merkle tree. 32 bytes max.
    pub salt: Box<[u8]>,
    /// The root hash of the merkle tree created.
    pub raw_root_hash: Box<[u8]>,
}

/// `SigningInfo` provides information that can be used to verify the idsig file.
#[derive(Default)]
pub struct SigningInfo {
    /// Digest of the APK that this idsig file is for.
    pub apk_digest: Box<[u8]>,
    /// Certificate of the signer that signed this idsig file. ASN.1 DER form.
    pub x509_certificate: Box<[u8]>,
    /// A free-form binary data
    pub additional_data: Box<[u8]>,
    /// Public key of the signer in ASN.1 DER form. This must match the `x509_certificate` field.
    pub public_key: Box<[u8]>,
    /// Signature algorithm used to sign this file.
    pub signature_algorithm_id: SignatureAlgorithmID,
    /// The signature of this file.
    pub signature: Box<[u8]>,
}

/// Version of the idsig file format
#[derive(Debug, PartialEq, Eq, FromPrimitive, ToPrimitive, Default)]
#[repr(u32)]
pub enum Version {
    #[default]
    /// Version 2, the only supported version.
    V2 = 2,
}

impl Version {
    fn from(val: u32) -> Result<Version> {
        Self::from_u32(val).ok_or_else(|| anyhow!("{} is an unsupported version", val))
    }
}

impl V4Signature<fs::File> {
    /// Creates a `V4Signature` struct from the given idsig path.
    pub fn from_idsig_path<P: AsRef<Path>>(idsig_path: P) -> Result<Self> {
        let idsig = fs::File::open(idsig_path).context("Cannot find idsig file")?;
        Self::from_idsig(idsig)
    }
}

impl<R: Read + Seek> V4Signature<R> {
    /// Consumes a stream for an idsig file into a `V4Signature` struct.
    pub fn from_idsig(mut r: R) -> Result<V4Signature<R>> {
        Ok(V4Signature {
            version: Version::from(r.read_u32::<LittleEndian>()?)?,
            hashing_info: HashingInfo::from(&mut r)?,
            signing_info: SigningInfo::from(&mut r)?,
            merkle_tree_size: r.read_u32::<LittleEndian>()?,
            merkle_tree_offset: r.stream_position()?,
            data: r,
        })
    }

    /// Read a stream for an APK file and creates a corresponding `V4Signature` struct that digests
    /// the APK file. Note that the signing is not done.
    /// Important: callers of this function are expected to verify the validity of the passed |apk|.
    /// To be more specific, they should check that |apk| corresponds to a regular file, as calling
    /// lseek on directory fds is not defined in the standard, and on ext4 it will return (off_t)-1
    /// (see: https://bugzilla.kernel.org/show_bug.cgi?id=200043), which will result in this
    /// function OOMing.
    pub fn create(
        mut apk: &mut R,
        current_sdk: u32,
        block_size: usize,
        salt: &[u8],
        algorithm: HashAlgorithm,
    ) -> Result<V4Signature<Cursor<Vec<u8>>>> {
        // Determine the size of the apk
        let start = apk.stream_position()?;
        let size = apk.seek(SeekFrom::End(0))? as usize;
        apk.seek(SeekFrom::Start(start))?;

        // Create hash tree (and root hash)
        let algorithm = match algorithm {
            HashAlgorithm::SHA256 => openssl::hash::MessageDigest::sha256(),
        };
        let hash_tree = HashTree::from(&mut apk, size, salt, block_size, algorithm)?;

        let mut ret = V4Signature {
            version: Version::default(),
            hashing_info: HashingInfo::default(),
            signing_info: SigningInfo::default(),
            merkle_tree_size: hash_tree.tree.len() as u32,
            merkle_tree_offset: 0, // merkle tree starts from the beginning of `data`
            data: Cursor::new(hash_tree.tree),
        };
        ret.hashing_info.raw_root_hash = hash_tree.root_hash.into_boxed_slice();
        ret.hashing_info.log2_blocksize = log2(block_size);

        apk.seek(SeekFrom::Start(start))?;
        let (signature_algorithm_id, apk_digest) =
            get_apk_digest(apk, current_sdk, /* verify= */ false)?;
        ret.signing_info.signature_algorithm_id = signature_algorithm_id;
        ret.signing_info.apk_digest = apk_digest;
        // TODO(jiyong): add a signature to the signing_info struct

        Ok(ret)
    }

    /// Writes the data into a writer
    pub fn write_into<W: Write + Seek>(&mut self, mut w: &mut W) -> Result<()> {
        // Writes the header part
        w.write_u32::<LittleEndian>(self.version.to_u32().unwrap())?;
        self.hashing_info.write_into(&mut w)?;
        self.signing_info.write_into(&mut w)?;
        w.write_u32::<LittleEndian>(self.merkle_tree_size)?;

        // Writes the merkle tree
        self.data.seek(SeekFrom::Start(self.merkle_tree_offset))?;
        let copied_size = copy(&mut self.data, &mut w)?;
        if copied_size != self.merkle_tree_size as u64 {
            bail!(
                "merkle tree is {} bytes, but only {} bytes are written.",
                self.merkle_tree_size,
                copied_size
            );
        }
        Ok(())
    }

    /// Returns the bytes that represents the merkle tree
    pub fn merkle_tree(&mut self) -> Result<Vec<u8>> {
        self.data.seek(SeekFrom::Start(self.merkle_tree_offset))?;
        let mut out = Vec::new();
        self.data.read_to_end(&mut out)?;
        Ok(out)
    }
}

impl HashingInfo {
    fn from(mut r: &mut dyn Read) -> Result<HashingInfo> {
        // Size of the entire hashing_info struct. We don't need this because each variable-sized
        // fields in the struct are also length encoded.
        r.read_u32::<LittleEndian>()?;
        Ok(HashingInfo {
            hash_algorithm: HashAlgorithm::from_read(&mut r)?,
            log2_blocksize: r.read_u8()?,
            salt: read_sized_array(&mut r)?,
            raw_root_hash: read_sized_array(&mut r)?,
        })
    }

    fn write_into<W: Write + Seek>(&self, mut w: &mut W) -> Result<()> {
        let start = w.stream_position()?;
        // Size of the entire hashing_info struct. Since we don't know the size yet, fill the place
        // with 0. The exact size will then be written below.
        w.write_u32::<LittleEndian>(0)?;

        w.write_u32::<LittleEndian>(self.hash_algorithm.to_u32().unwrap())?;
        w.write_u8(self.log2_blocksize)?;
        write_sized_array(&mut w, &self.salt)?;
        write_sized_array(&mut w, &self.raw_root_hash)?;

        // Determine the size of hashing_info, and write it in front of the struct where the value
        // was initialized to zero.
        let end = w.stream_position()?;
        let size = end - start - std::mem::size_of::<u32>() as u64;
        w.seek(SeekFrom::Start(start))?;
        w.write_u32::<LittleEndian>(size as u32)?;
        w.seek(SeekFrom::Start(end))?;
        Ok(())
    }
}

impl SigningInfo {
    fn from(mut r: &mut dyn Read) -> Result<SigningInfo> {
        // Size of the entire signing_info struct. We don't need this because each variable-sized
        // fields in the struct are also length encoded.
        r.read_u32::<LittleEndian>()?;
        Ok(SigningInfo {
            apk_digest: read_sized_array(&mut r)?,
            x509_certificate: read_sized_array(&mut r)?,
            additional_data: read_sized_array(&mut r)?,
            public_key: read_sized_array(&mut r)?,
            signature_algorithm_id: SignatureAlgorithmID::from_u32(r.read_u32::<LittleEndian>()?)
                .context("Unsupported signature algorithm")?,
            signature: read_sized_array(&mut r)?,
        })
    }

    fn write_into<W: Write + Seek>(&self, mut w: &mut W) -> Result<()> {
        let start = w.stream_position()?;
        // Size of the entire signing_info struct. Since we don't know the size yet, fill the place
        // with 0. The exact size will then be written below.
        w.write_u32::<LittleEndian>(0)?;

        write_sized_array(&mut w, &self.apk_digest)?;
        write_sized_array(&mut w, &self.x509_certificate)?;
        write_sized_array(&mut w, &self.additional_data)?;
        write_sized_array(&mut w, &self.public_key)?;
        w.write_u32::<LittleEndian>(self.signature_algorithm_id.to_u32())?;
        write_sized_array(&mut w, &self.signature)?;

        // Determine the size of signing_info, and write it in front of the struct where the value
        // was initialized to zero.
        let end = w.stream_position()?;
        let size = end - start - std::mem::size_of::<u32>() as u64;
        w.seek(SeekFrom::Start(start))?;
        w.write_u32::<LittleEndian>(size as u32)?;
        w.seek(SeekFrom::Start(end))?;
        Ok(())
    }
}

fn read_sized_array(r: &mut dyn Read) -> Result<Box<[u8]>> {
    let size = r.read_u32::<LittleEndian>()?;
    let mut data = vec![0; size as usize];
    r.read_exact(&mut data)?;
    Ok(data.into_boxed_slice())
}

fn write_sized_array(w: &mut dyn Write, data: &[u8]) -> Result<()> {
    w.write_u32::<LittleEndian>(data.len() as u32)?;
    Ok(w.write_all(data)?)
}

fn log2(n: usize) -> u8 {
    let num_bits = std::mem::size_of::<usize>() * 8;
    (num_bits as u32 - n.leading_zeros() - 1) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const TEST_APK_PATH: &str = "tests/data/v4-digest-v3-Sha256withEC.apk";

    #[test]
    fn parse_idsig_file() {
        let parsed = V4Signature::from_idsig_path(format!("{TEST_APK_PATH}.idsig")).unwrap();

        assert_eq!(Version::V2, parsed.version);

        let hi = parsed.hashing_info;
        assert_eq!(HashAlgorithm::SHA256, hi.hash_algorithm);
        assert_eq!(12, hi.log2_blocksize);
        assert_eq!("", hex::encode(hi.salt.as_ref()));
        assert_eq!(
            "77f063b48b63f846690fa76450a8d3b61a295b6158f50592e873f76dbeeb0201",
            hex::encode(hi.raw_root_hash.as_ref())
        );

        let si = parsed.signing_info;
        assert_eq!(
            "c02fe2eddeb3078801828b930de546ea4f98d37fb98b40c7c7ed169b0d713583",
            hex::encode(si.apk_digest.as_ref())
        );
        assert_eq!("", hex::encode(si.additional_data.as_ref()));
        assert_eq!(
            "3046022100fb6383ba300dc7e1e6931a25b381398a16e5575baefd82afd12ba88660d9a6\
            4c022100ebdcae13ab18c4e30bf6ae634462e526367e1ba26c2647a1d87a0f42843fc128",
            hex::encode(si.signature.as_ref())
        );
        assert_eq!(SignatureAlgorithmID::EcdsaWithSha256, si.signature_algorithm_id);

        assert_eq!(4096, parsed.merkle_tree_size);
        assert_eq!(648, parsed.merkle_tree_offset);
    }

    /// Parse an idsig file into V4Signature and write it. The written date must be the same as
    /// the input file.
    #[test]
    fn parse_and_compose() {
        let idsig_path = format!("{TEST_APK_PATH}.idsig");
        let mut v4_signature = V4Signature::from_idsig_path(&idsig_path).unwrap();

        let mut output = Cursor::new(Vec::new());
        v4_signature.write_into(&mut output).unwrap();

        assert_eq!(fs::read(&idsig_path).unwrap(), output.get_ref().as_slice());
    }

    /// Create V4Signature by hashing an APK. Merkle tree and the root hash should be the same
    /// as those in the idsig file created by the signapk tool.
    #[test]
    fn digest_from_apk() {
        let mut input = Cursor::new(include_bytes!("../tests/data/v4-digest-v3-Sha256withEC.apk"));
        let current_sdk = 31;
        let mut created =
            V4Signature::create(&mut input, current_sdk, 4096, &[], HashAlgorithm::SHA256).unwrap();

        let mut golden = V4Signature::from_idsig_path(format!("{TEST_APK_PATH}.idsig")).unwrap();

        // Compare the root hash
        assert_eq!(
            created.hashing_info.raw_root_hash.as_ref(),
            golden.hashing_info.raw_root_hash.as_ref()
        );

        // Compare the merkle tree
        assert_eq!(
            created.merkle_tree().unwrap().as_slice(),
            golden.merkle_tree().unwrap().as_slice()
        );
    }
}
