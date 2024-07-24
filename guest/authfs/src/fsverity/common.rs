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

use std::io;

use thiserror::Error;

use super::sys::{FS_VERITY_HASH_ALG_SHA256, FS_VERITY_LOG_BLOCKSIZE, FS_VERITY_VERSION};
use crate::common::{divide_roundup, CHUNK_SIZE};
use openssl::sha::Sha256;

/// Output size of SHA-256 in bytes.
pub const SHA256_HASH_SIZE: usize = 32;

/// A SHA-256 hash.
pub type Sha256Hash = [u8; SHA256_HASH_SIZE];

#[derive(Error, Debug)]
pub enum FsverityError {
    #[error("Invalid digest")]
    InvalidDigest,
    #[error("Insufficient data, only got {0}")]
    InsufficientData(usize),
    #[error("Cannot verify a block")]
    CannotVerify,
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("Invalid state")]
    InvalidState,
}

fn log128_ceil(num: u64) -> Option<u64> {
    match num {
        0 => None,
        n => Some(divide_roundup(64 - (n - 1).leading_zeros() as u64, 7)),
    }
}

/// Return the Merkle tree height for our tree configuration, or None if the size is 0.
pub fn merkle_tree_height(data_size: u64) -> Option<u64> {
    let hashes_per_node = CHUNK_SIZE / SHA256_HASH_SIZE as u64;
    let hash_pages = divide_roundup(data_size, hashes_per_node * CHUNK_SIZE);
    log128_ceil(hash_pages)
}

/// Returns the size of Merkle tree for `data_size` bytes amount of data.
pub fn merkle_tree_size(mut data_size: u64) -> u64 {
    let mut total = 0;
    while data_size > CHUNK_SIZE {
        let hash_size = divide_roundup(data_size, CHUNK_SIZE) * SHA256_HASH_SIZE as u64;
        let hash_storage_size = divide_roundup(hash_size, CHUNK_SIZE) * CHUNK_SIZE;
        total += hash_storage_size;
        data_size = hash_storage_size;
    }
    total
}

pub fn build_fsverity_digest(root_hash: &Sha256Hash, file_size: u64) -> Sha256Hash {
    // Little-endian byte representation of fsverity_descriptor from linux/fsverity.h
    // Not FFI-ed as it seems easier to deal with the raw bytes manually.
    let mut hash = Sha256::new();
    hash.update(&FS_VERITY_VERSION.to_le_bytes()); // version
    hash.update(&FS_VERITY_HASH_ALG_SHA256.to_le_bytes()); // hash_algorithm
    hash.update(&FS_VERITY_LOG_BLOCKSIZE.to_le_bytes()); // log_blocksize
    hash.update(&0u8.to_le_bytes()); // salt_size
    hash.update(&0u32.to_le_bytes()); // sig_size
    hash.update(&file_size.to_le_bytes()); // data_size
    hash.update(root_hash); // root_hash, first 32 bytes
    hash.update(&[0u8; 32]); // root_hash, last 32 bytes, always 0 because we are using sha256.
    hash.update(&[0u8; 32]); // salt
    hash.update(&[0u8; 32]); // reserved
    hash.update(&[0u8; 32]); // reserved
    hash.update(&[0u8; 32]); // reserved
    hash.update(&[0u8; 32]); // reserved
    hash.update(&[0u8; 16]); // reserved
    hash.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_tree_size() {
        // To produce groundtruth:
        //   dd if=/dev/zero of=zeros bs=1 count=524289 && \
        //   fsverity digest --out-merkle-tree=tree zeros && \
        //   du -b tree
        assert_eq!(merkle_tree_size(0), 0);
        assert_eq!(merkle_tree_size(1), 0);
        assert_eq!(merkle_tree_size(4096), 0);
        assert_eq!(merkle_tree_size(4097), 4096);
        assert_eq!(merkle_tree_size(524288), 4096);
        assert_eq!(merkle_tree_size(524289), 12288);
    }
}
