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

use super::common::{
    build_fsverity_digest, merkle_tree_height, FsverityError, Sha256Hash, SHA256_HASH_SIZE,
};
use crate::common::{divide_roundup, CHUNK_SIZE};
use openssl::sha::Sha256;

const HASH_SIZE: usize = SHA256_HASH_SIZE;
const HASH_PER_PAGE: usize = CHUNK_SIZE as usize / HASH_SIZE;

const HASH_OF_4096_ZEROS: Sha256Hash = [
    0xad, 0x7f, 0xac, 0xb2, 0x58, 0x6f, 0xc6, 0xe9, 0x66, 0xc0, 0x04, 0xd7, 0xd1, 0xd1, 0x6b, 0x02,
    0x4f, 0x58, 0x05, 0xff, 0x7c, 0xb4, 0x7c, 0x7a, 0x85, 0xda, 0xbd, 0x8b, 0x48, 0x89, 0x2c, 0xa7,
];

/// MerkleLeaves can be used by the class' customer for bookkeeping integrity data for their bytes.
/// It can also be used to generate the standard fs-verity digest for the source data.
///
/// It's in-memory because for the initial use cases, we don't need to read back an existing file,
/// and only need to deal with new files. Also, considering that the output file won't be large at
/// the moment, it is sufficient to simply keep the Merkle tree in memory in the trusted world. To
/// further simplify the initial implementation, we only need to keep the leaf nodes in memory, and
/// generate the tree / root hash when requested.
pub struct MerkleLeaves {
    leaves: Vec<Sha256Hash>,
    file_size: u64,
}

fn hash_all_pages(source: &[Sha256Hash]) -> Vec<Sha256Hash> {
    source
        .chunks(HASH_PER_PAGE)
        .map(|chunk| {
            let padding_bytes = (HASH_PER_PAGE - chunk.len()) * HASH_SIZE;
            let mut ctx = Sha256::new();
            for data in chunk {
                ctx.update(data.as_ref());
            }
            ctx.update(&vec![0u8; padding_bytes]);
            ctx.finish()
        })
        .collect()
}

impl MerkleLeaves {
    /// Creates a `MerkleLeaves` instance with empty data.
    pub fn new() -> Self {
        Self { leaves: Vec::new(), file_size: 0 }
    }

    /// Gets size of the file represented by `MerkleLeaves`.
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Grows the `MerkleLeaves` to the new file size. To keep the consistency, if any new leaves
    /// are added, the leaves/hashes are as if the extended content is all zero.
    ///
    /// However, when the change shrinks the leaf number, `MerkleLeaves` does not know if the hash
    /// of the last chunk has changed, or what the new value should be. As the result, it is up to
    /// the caller to fix the last leaf if needed.
    pub fn resize(&mut self, new_file_size: usize) {
        let new_file_size = new_file_size as u64;
        let leaves_size = divide_roundup(new_file_size, CHUNK_SIZE);
        self.leaves.resize(leaves_size as usize, HASH_OF_4096_ZEROS);
        self.file_size = new_file_size;
    }

    /// Updates the hash of the `index`-th leaf, and increase the size to `size_at_least` if the
    /// current size is smaller.
    pub fn update_hash(&mut self, index: usize, hash: &Sha256Hash, size_at_least: u64) {
        // +1 since index is zero-based.
        if self.leaves.len() < index + 1 {
            // When resizing, fill in hash of zeros by default. This makes it easy to handle holes
            // in a file.
            self.leaves.resize(index + 1, HASH_OF_4096_ZEROS);
        }
        self.leaves[index].clone_from_slice(hash);

        if size_at_least > self.file_size {
            self.file_size = size_at_least;
        }
    }

    /// Returns whether `index` is within the bound of leaves.
    pub fn is_index_valid(&self, index: usize) -> bool {
        index < self.leaves.len()
    }

    /// Returns whether the `index`-th hash is consistent to `hash`.
    pub fn is_consistent(&self, index: usize, hash: &Sha256Hash) -> bool {
        if let Some(element) = self.leaves.get(index) {
            element == hash
        } else {
            false
        }
    }

    fn calculate_root_hash(&self) -> Result<Sha256Hash, FsverityError> {
        match self.leaves.len() {
            // Special cases per fs-verity digest definition.
            0 => {
                debug_assert_eq!(self.file_size, 0);
                Ok([0u8; HASH_SIZE])
            }
            1 => {
                debug_assert!(self.file_size <= CHUNK_SIZE && self.file_size > 0);
                Ok(self.leaves[0])
            }
            n => {
                debug_assert_eq!((self.file_size - 1) / CHUNK_SIZE, n as u64);
                let size_for_equivalent = n as u64 * CHUNK_SIZE;
                let level = merkle_tree_height(size_for_equivalent).unwrap(); // safe since n > 0

                // `leaves` is owned and can't be the initial state below. Here we manually hash it
                // first to avoid a copy and to get the type right.
                let second_level = hash_all_pages(&self.leaves);
                let hashes = (1..=level).fold(second_level, |source, _| hash_all_pages(&source));
                if hashes.len() != 1 {
                    Err(FsverityError::InvalidState)
                } else {
                    Ok(hashes.into_iter().next().unwrap())
                }
            }
        }
    }

    /// Returns the fs-verity digest based on the current tree and file size.
    pub fn calculate_fsverity_digest(&self) -> Result<Sha256Hash, FsverityError> {
        let root_hash = self.calculate_root_hash()?;
        Ok(build_fsverity_digest(&root_hash, self.file_size))
    }
}

#[cfg(test)]
mod tests {
    // Test data below can be generated by:
    //  $ perl -e 'print "\x{00}" x 6000' > foo
    //  $ perl -e 'print "\x{01}" x 5000' >> foo
    //  $ fsverity digest foo
    use super::*;
    use anyhow::Result;
    use openssl::sha::sha256;

    #[test]
    fn merkle_tree_empty_file() -> Result<()> {
        assert_eq!(
            hex::decode("3d248ca542a24fc62d1c43b916eae5016878e2533c88238480b26128a1f1af95")?,
            generate_fsverity_digest_sequentially(&Vec::new())?
        );
        Ok(())
    }

    #[test]
    fn merkle_tree_file_size_less_than_or_equal_to_4k() -> Result<()> {
        // Test a file that contains 4096 '\01's.
        assert_eq!(
            hex::decode("cd0875ca59c7d37e962c5e8f5acd3770750ac80225e2df652ce5672fd34500af")?,
            generate_fsverity_digest_sequentially(&vec![1; 4096])?
        );
        Ok(())
    }

    #[test]
    fn merkle_tree_more_sizes() -> Result<()> {
        // Test files that contains >4096 '\01's.

        assert_eq!(
            hex::decode("2901b849fda2d91e3929524561c4a47e77bb64734319759507b2029f18b9cc52")?,
            generate_fsverity_digest_sequentially(&vec![1; 4097])?
        );

        assert_eq!(
            hex::decode("2a476d58eb80394052a3a783111e1458ac3ecf68a7878183fed86ca0ff47ec0d")?,
            generate_fsverity_digest_sequentially(&vec![1; 8192])?
        );

        // Test with max size that still fits in 2 levels.
        assert_eq!(
            hex::decode("26b7c190a34e19f420808ee7ec233b09fa6c34543b5a9d2950530114c205d14f")?,
            generate_fsverity_digest_sequentially(&vec![1; 524288])?
        );

        // Test with data that requires 3 levels.
        assert_eq!(
            hex::decode("316835d9be1c95b5cd55d07ae7965d651689efad186e26cbf680e40b683a3262")?,
            generate_fsverity_digest_sequentially(&vec![1; 524289])?
        );
        Ok(())
    }

    #[test]
    fn merkle_tree_non_sequential() -> Result<()> {
        let mut tree = MerkleLeaves::new();
        let hash = sha256(&vec![1u8; CHUNK_SIZE as usize]);

        // Update hashes of 4 1-blocks.
        tree.update_hash(1, &hash, CHUNK_SIZE * 2);
        tree.update_hash(3, &hash, CHUNK_SIZE * 4);
        tree.update_hash(0, &hash, CHUNK_SIZE);
        tree.update_hash(2, &hash, CHUNK_SIZE * 3);

        assert_eq!(
            hex::decode("7d3c0d2e1dc54230b20ed875f5f3a4bd3f9873df601936b3ca8127d4db3548f3")?,
            tree.calculate_fsverity_digest()?
        );
        Ok(())
    }

    #[test]
    fn merkle_tree_grow_leaves() -> Result<()> {
        let mut tree = MerkleLeaves::new();
        tree.update_hash(0, &[42; HASH_SIZE], CHUNK_SIZE); // fake hash of a CHUNK_SIZE block

        // Grows the leaves
        tree.resize(4096 * 3 - 100);

        assert!(tree.is_index_valid(0));
        assert!(tree.is_index_valid(1));
        assert!(tree.is_index_valid(2));
        assert!(!tree.is_index_valid(3));
        assert!(tree.is_consistent(1, &HASH_OF_4096_ZEROS));
        assert!(tree.is_consistent(2, &HASH_OF_4096_ZEROS));
        Ok(())
    }

    #[test]
    fn merkle_tree_shrink_leaves() -> Result<()> {
        let mut tree = MerkleLeaves::new();
        tree.update_hash(0, &[42; HASH_SIZE], CHUNK_SIZE);
        tree.update_hash(0, &[42; HASH_SIZE], CHUNK_SIZE * 3);

        // Shrink the leaves
        tree.resize(CHUNK_SIZE as usize * 2 - 100);

        assert!(tree.is_index_valid(0));
        assert!(tree.is_index_valid(1));
        assert!(!tree.is_index_valid(2));
        // The second chunk is a hole and full of zero. When shrunk, with zero padding, the hash
        // happens to be consistent to a full-zero chunk.
        assert!(tree.is_consistent(1, &HASH_OF_4096_ZEROS));
        Ok(())
    }

    fn generate_fsverity_digest_sequentially(test_data: &[u8]) -> Result<Sha256Hash> {
        let mut tree = MerkleLeaves::new();
        for (index, chunk) in test_data.chunks(CHUNK_SIZE as usize).enumerate() {
            let mut ctx = Sha256::new();
            ctx.update(chunk);
            ctx.update(&vec![0u8; CHUNK_SIZE as usize - chunk.len()]);
            let hash = ctx.finish();

            tree.update_hash(index, &hash, CHUNK_SIZE * index as u64 + chunk.len() as u64);
        }
        Ok(tree.calculate_fsverity_digest()?)
    }
}
