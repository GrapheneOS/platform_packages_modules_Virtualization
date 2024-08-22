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

//! Utilities for Signature Verification

use anyhow::{anyhow, ensure, Error, Result};
use apkzip::{set_central_directory_offset, zip_sections};
use byteorder::{LittleEndian, ReadBytesExt};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use openssl::hash::{DigestBytes, Hasher, MessageDigest};
use std::cmp::min;
use std::io::{self, Cursor, ErrorKind, Read, Seek, SeekFrom, Take};

use crate::algorithms::SignatureAlgorithmID;

const APK_SIG_BLOCK_MIN_SIZE: u32 = 32;
const APK_SIG_BLOCK_MAGIC: u128 = 0x3234206b636f6c4220676953204b5041;

const CHUNK_SIZE_BYTES: u64 = 1024 * 1024;
const CHUNK_HEADER_TOP: &[u8] = &[0x5a];
const CHUNK_HEADER_MID: &[u8] = &[0xa5];

/// The [APK structure] has four major sections:
///
/// | Zip contents | APK Signing Block | Central directory | EOCD(End of Central Directory) |
///
/// This structure contains the offset/size information of all the sections except the Zip contents.
///
/// [APK structure]: https://source.android.com/docs/security/apksigning/v2#apk-signing-block
pub struct ApkSections<R> {
    inner: R,
    signing_block_offset: u32,
    signing_block_size: u32,
    central_directory_offset: u32,
    central_directory_size: u32,
    eocd_offset: u32,
    eocd_size: u32,
}

impl<R: Read + Seek> ApkSections<R> {
    pub fn new(mut reader: R) -> Result<ApkSections<R>> {
        let zip_sections = zip_sections(&mut reader)?;
        let (signing_block_offset, signing_block_size) =
            find_signing_block(&mut reader, zip_sections.central_directory_offset)?;
        Ok(ApkSections {
            inner: reader,
            signing_block_offset,
            signing_block_size,
            central_directory_offset: zip_sections.central_directory_offset,
            central_directory_size: zip_sections.central_directory_size,
            eocd_offset: zip_sections.eocd_offset,
            eocd_size: zip_sections.eocd_size,
        })
    }

    /// Returns the APK Signature Scheme block contained in the provided file for the given ID
    /// and the additional information relevant for verifying the block against the file.
    pub fn find_signature(&mut self, block_id: u32) -> Result<Bytes> {
        let signing_block = self.bytes(self.signing_block_offset, self.signing_block_size)?;
        find_signature_scheme_block(Bytes::from(signing_block), block_id)
    }

    /// Computes digest with "signature algorithm" over APK contents, central directory, and EOCD.
    /// 1. The digest of each chunk is computed over the concatenation of byte 0xa5, the chunk’s
    ///    length in bytes (little-endian uint32), and the chunk’s contents.
    /// 2. The top-level digest is computed over the concatenation of byte 0x5a, the number of
    ///    chunks (little-endian uint32), and the concatenation of digests of the chunks in the
    ///    order the chunks appear in the APK.
    ///
    /// (see https://source.android.com/security/apksigning/v2#integrity-protected-contents)
    pub(crate) fn compute_digest(
        &mut self,
        signature_algorithm_id: SignatureAlgorithmID,
    ) -> Result<Vec<u8>> {
        let digester = Digester { message_digest: signature_algorithm_id.new_message_digest() };
        let mut digests_of_chunks = BytesMut::new();
        let mut chunk_count = 0u32;
        let mut chunk = vec![0u8; CHUNK_SIZE_BYTES as usize];
        for data in &[
            ApkSections::zip_entries,
            ApkSections::central_directory,
            ApkSections::eocd_for_verification,
        ] {
            let mut data = data(self)?;
            while data.limit() > 0 {
                let chunk_size = min(CHUNK_SIZE_BYTES, data.limit());
                let slice = &mut chunk[..(chunk_size as usize)];
                data.read_exact(slice)?;
                digests_of_chunks.put_slice(
                    digester.digest(slice, CHUNK_HEADER_MID, chunk_size as u32)?.as_ref(),
                );
                chunk_count += 1;
            }
        }
        Ok(digester.digest(&digests_of_chunks, CHUNK_HEADER_TOP, chunk_count)?.as_ref().into())
    }

    fn zip_entries(&mut self) -> Result<Take<Box<dyn Read + '_>>> {
        scoped_read(&mut self.inner, 0, self.signing_block_offset as u64)
    }

    fn central_directory(&mut self) -> Result<Take<Box<dyn Read + '_>>> {
        scoped_read(
            &mut self.inner,
            self.central_directory_offset as u64,
            self.central_directory_size as u64,
        )
    }

    fn eocd_for_verification(&mut self) -> Result<Take<Box<dyn Read + '_>>> {
        let mut eocd = self.bytes(self.eocd_offset, self.eocd_size)?;
        // Protection of section 4 (ZIP End of Central Directory) is complicated by the section
        // containing the offset of ZIP Central Directory. The offset changes when the size of the
        // APK Signing Block changes, for instance, when a new signature is added. Thus, when
        // computing digest over the ZIP End of Central Directory, the field containing the offset
        // of ZIP Central Directory must be treated as containing the offset of the APK Signing
        // Block.
        set_central_directory_offset(&mut eocd, self.signing_block_offset)?;
        Ok(Read::take(Box::new(Cursor::new(eocd)), self.eocd_size as u64))
    }

    fn bytes(&mut self, offset: u32, size: u32) -> Result<Vec<u8>> {
        self.inner.seek(SeekFrom::Start(offset as u64))?;
        let mut buf = vec![0u8; size as usize];
        self.inner.read_exact(&mut buf)?;
        Ok(buf)
    }
}

fn scoped_read<'a, R: Read + Seek>(
    src: &'a mut R,
    offset: u64,
    size: u64,
) -> Result<Take<Box<dyn Read + 'a>>> {
    src.seek(SeekFrom::Start(offset))?;
    Ok(Read::take(Box::new(src), size))
}

struct Digester {
    message_digest: MessageDigest,
}

impl Digester {
    // v2/v3 digests are computed after prepending "header" byte and "size" info.
    fn digest(&self, data: &[u8], header: &[u8], size: u32) -> Result<DigestBytes> {
        let mut hasher = Hasher::new(self.message_digest)?;
        hasher.update(header)?;
        hasher.update(&size.to_le_bytes())?;
        hasher.update(data)?;
        Ok(hasher.finish()?)
    }
}

fn find_signing_block<T: Read + Seek>(
    reader: &mut T,
    central_directory_offset: u32,
) -> Result<(u32, u32)> {
    // FORMAT:
    // OFFSET       DATA TYPE  DESCRIPTION
    // * @+0  bytes uint64:    size in bytes (excluding this field)
    // * @+8  bytes payload
    // * @-24 bytes uint64:    size in bytes (same as the one above)
    // * @-16 bytes uint128:   magic
    ensure!(
        central_directory_offset >= APK_SIG_BLOCK_MIN_SIZE,
        "APK too small for APK Signing Block. ZIP Central Directory offset: {}",
        central_directory_offset
    );
    reader.seek(SeekFrom::Start((central_directory_offset - 24) as u64))?;
    let size_in_footer = reader.read_u64::<LittleEndian>()? as u32;
    ensure!(
        reader.read_u128::<LittleEndian>()? == APK_SIG_BLOCK_MAGIC,
        "No APK Signing Block before ZIP Central Directory"
    );
    let total_size = size_in_footer + 8;
    let signing_block_offset = central_directory_offset
        .checked_sub(total_size)
        .ok_or_else(|| anyhow!("APK Signing Block size out of range: {}", size_in_footer))?;
    reader.seek(SeekFrom::Start(signing_block_offset as u64))?;
    let size_in_header = reader.read_u64::<LittleEndian>()? as u32;
    // This corresponds to APK Signature Scheme v3 verification step 1a.
    ensure!(
        size_in_header == size_in_footer,
        "APK Signing Block sizes in header and footer do not match: {} vs {}",
        size_in_header,
        size_in_footer
    );
    Ok((signing_block_offset, total_size))
}

fn find_signature_scheme_block(buf: Bytes, block_id: u32) -> Result<Bytes> {
    // FORMAT:
    // OFFSET       DATA TYPE  DESCRIPTION
    // * @+0  bytes uint64:    size in bytes (excluding this field)
    // * @+8  bytes pairs
    // * @-24 bytes uint64:    size in bytes (same as the one above)
    // * @-16 bytes uint128:   magic
    let mut pairs = buf.slice(8..(buf.len() - 24));
    let mut entry_count = 0;
    while pairs.has_remaining() {
        entry_count += 1;
        ensure!(
            pairs.remaining() >= 8,
            "Insufficient data to read size of APK Signing Block entry #{}",
            entry_count
        );
        let length = pairs.get_u64_le();
        let mut pair = pairs.split_to(length as usize);
        let id = pair.get_u32_le();
        if id == block_id {
            return Ok(pair);
        }
    }
    let context =
        format!("No APK Signature Scheme block in APK Signing Block with ID: {}", block_id);
    Err(Error::new(io::Error::from(ErrorKind::NotFound)).context(context))
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::LittleEndian;
    use std::fs::File;
    use std::mem::size_of_val;

    use crate::v3::APK_SIGNATURE_SCHEME_V3_BLOCK_ID;

    const CENTRAL_DIRECTORY_HEADER_SIGNATURE: u32 = 0x02014b50;

    #[test]
    fn test_apk_sections() {
        let apk_file = File::open("tests/data/v3-only-with-ecdsa-sha512-p521.apk").unwrap();
        let apk_sections = ApkSections::new(apk_file).unwrap();
        let mut reader = &apk_sections.inner;

        // Checks APK Signing Block.
        assert_eq!(
            apk_sections.signing_block_offset + apk_sections.signing_block_size,
            apk_sections.central_directory_offset
        );
        let apk_signature_offset = SeekFrom::Start(
            apk_sections.central_directory_offset as u64 - size_of_val(&APK_SIG_BLOCK_MAGIC) as u64,
        );
        reader.seek(apk_signature_offset).unwrap();
        assert_eq!(reader.read_u128::<LittleEndian>().unwrap(), APK_SIG_BLOCK_MAGIC);

        // Checks Central directory.
        assert_eq!(reader.read_u32::<LittleEndian>().unwrap(), CENTRAL_DIRECTORY_HEADER_SIGNATURE);
        assert_eq!(
            apk_sections.central_directory_offset + apk_sections.central_directory_size,
            apk_sections.eocd_offset
        );

        // Checks EOCD.
        assert_eq!(
            reader.metadata().unwrap().len(),
            (apk_sections.eocd_offset + apk_sections.eocd_size) as u64
        );
    }

    #[test]
    fn test_apk_digest() {
        let apk_file = File::open("tests/data/v3-only-with-dsa-sha256-1024.apk").unwrap();
        let mut apk_sections = ApkSections::new(apk_file).unwrap();
        let digest = apk_sections.compute_digest(SignatureAlgorithmID::DsaWithSha256).unwrap();
        assert_eq!(
            "0df2426ea33aedaf495d88e5be0c6a1663ff0a81c5ed12d5b2929ae4b4300f2f",
            hex::encode(&digest[..])
        );
    }

    #[test]
    fn test_apk_sections_cannot_find_signature() {
        let apk_file = File::open("tests/data/v2-only-two-signers.apk").unwrap();
        let mut apk_sections = ApkSections::new(apk_file).unwrap();
        let result = apk_sections.find_signature(APK_SIGNATURE_SCHEME_V3_BLOCK_ID);

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.downcast_ref::<io::Error>().unwrap().kind(), ErrorKind::NotFound);
        assert!(
            error.to_string().contains(&APK_SIGNATURE_SCHEME_V3_BLOCK_ID.to_string()),
            "Error should contain the block ID: {}",
            error
        );
    }

    #[test]
    fn test_apk_sections_find_signature() {
        let apk_file = File::open("tests/data/v3-only-with-dsa-sha256-1024.apk").unwrap();
        let mut apk_sections = ApkSections::new(apk_file).unwrap();
        let signature = apk_sections.find_signature(APK_SIGNATURE_SCHEME_V3_BLOCK_ID).unwrap();

        let expected_v3_signature_block_size = 1289; // Only for this specific APK
        assert_eq!(signature.len(), expected_v3_signature_block_size);
    }
}
