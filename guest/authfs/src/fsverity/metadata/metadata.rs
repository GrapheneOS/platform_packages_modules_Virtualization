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

//! Rust bindgen interface for FSVerity Metadata file (.fsv_meta)
use authfs_fsverity_metadata_bindgen::{
    fsverity_descriptor, fsverity_metadata_header, FSVERITY_HASH_ALG_SHA256,
    FSVERITY_SIGNATURE_TYPE_NONE, FSVERITY_SIGNATURE_TYPE_PKCS7, FSVERITY_SIGNATURE_TYPE_RAW,
};

use openssl::sha::sha256;
use std::cmp::min;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::mem::{size_of, zeroed};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::slice::from_raw_parts_mut;

/// Offset of `descriptor` in `struct fsverity_metadatata_header`.
const DESCRIPTOR_OFFSET: usize = 4;

/// Structure for parsed metadata.
pub struct FSVerityMetadata {
    /// Header for the metadata.
    pub header: fsverity_metadata_header,

    /// fs-verity digest of the file, with hash algorithm defined in the fs-verity descriptor.
    pub digest: Vec<u8>,

    /// Optional signature for the metadata.
    pub signature: Option<Vec<u8>>,

    metadata_file: File,

    merkle_tree_offset: u64,
}

impl FSVerityMetadata {
    /// Read the raw Merkle tree from the metadata, if it exists. The API semantics is similar to a
    /// regular pread(2), and may not return full requested buffer.
    pub fn read_merkle_tree(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let file_size = self.metadata_file.metadata()?.size();
        let start = self.merkle_tree_offset + offset;
        let end = min(file_size, start + buf.len() as u64);
        let read_size = (end - start) as usize;
        debug_assert!(read_size <= buf.len());
        if read_size == 0 {
            Ok(0)
        } else {
            self.metadata_file.read_exact_at(&mut buf[..read_size], start)?;
            Ok(read_size)
        }
    }
}

/// Common block and page size in Linux.
pub const CHUNK_SIZE: u64 = authfs_fsverity_metadata_bindgen::CHUNK_SIZE;

/// Derive a path of metadata for a given path.
/// e.g. "system/framework/foo.jar" -> "system/framework/foo.jar.fsv_meta"
pub fn get_fsverity_metadata_path(path: &Path) -> PathBuf {
    let mut os_string: OsString = path.into();
    os_string.push(".fsv_meta");
    os_string.into()
}

/// Parse metadata from given file, and returns a structure for the metadata.
pub fn parse_fsverity_metadata(mut metadata_file: File) -> io::Result<Box<FSVerityMetadata>> {
    let (header, digest) = {
        // SAFETY: The header doesn't include any pointers.
        let mut header: fsverity_metadata_header = unsafe { zeroed() };

        // SAFETY: fsverity_metadata_header is packed, so reading/write from/to the back_buffer
        // won't overflow.
        let back_buffer = unsafe {
            from_raw_parts_mut(
                &mut header as *mut fsverity_metadata_header as *mut u8,
                size_of::<fsverity_metadata_header>(),
            )
        };
        metadata_file.read_exact(back_buffer)?;

        // Digest needs to be calculated with the raw value (without changing the endianness).
        let digest = match header.descriptor.hash_algorithm {
            FSVERITY_HASH_ALG_SHA256 => Ok(sha256(
                &back_buffer
                    [DESCRIPTOR_OFFSET..DESCRIPTOR_OFFSET + size_of::<fsverity_descriptor>()],
            )
            .to_vec()),
            alg => Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Unsupported hash algorithm {}, continue (likely failing soon)", alg),
            )),
        }?;

        // TODO(inseob): This doesn't seem ideal. Maybe we can consider nom?
        header.version = u32::from_le(header.version);
        header.descriptor.data_size = u64::from_le(header.descriptor.data_size);
        header.signature_type = u32::from_le(header.signature_type);
        header.signature_size = u32::from_le(header.signature_size);
        (header, digest)
    };

    if header.version != 1 {
        return Err(io::Error::new(io::ErrorKind::Other, "unsupported metadata version"));
    }

    let signature = match header.signature_type {
        FSVERITY_SIGNATURE_TYPE_NONE => None,
        FSVERITY_SIGNATURE_TYPE_PKCS7 | FSVERITY_SIGNATURE_TYPE_RAW => {
            // TODO: unpad pkcs7?
            let mut buf = vec![0u8; header.signature_size as usize];
            metadata_file.read_exact(&mut buf)?;
            Some(buf)
        }
        _ => return Err(io::Error::new(io::ErrorKind::Other, "unknown signature type")),
    };

    // merkle tree is at the next 4K boundary
    let merkle_tree_offset =
        (metadata_file.stream_position()? + CHUNK_SIZE - 1) / CHUNK_SIZE * CHUNK_SIZE;

    Ok(Box::new(FSVerityMetadata { header, digest, signature, metadata_file, merkle_tree_offset }))
}
