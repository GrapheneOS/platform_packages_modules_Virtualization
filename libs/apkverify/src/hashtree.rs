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

use openssl::hash::{DigestBytes, Hasher, MessageDigest};
use std::io::{Cursor, Read, Result, Write};

/// `HashTree` is a merkle tree (and its root hash) that is compatible with fs-verity.
pub struct HashTree {
    /// Binary presentation of the merkle tree
    pub tree: Vec<u8>,
    /// Root hash
    pub root_hash: Vec<u8>,
}

impl HashTree {
    /// Creates merkle tree from `input`, using the given `salt` and hashing `algorithm`. `input`
    /// is divided into `block_size` chunks.
    pub fn from<R: Read>(
        input: &mut R,
        input_size: usize,
        salt: &[u8],
        block_size: usize,
        algorithm: MessageDigest,
    ) -> Result<Self> {
        let salt = zero_pad_salt(salt, algorithm);
        let tree = generate_hash_tree(input, input_size, &salt, block_size, algorithm)?;

        // Root hash is from the first block of the hash or the input data if there is no hash tree
        // generated which can happen when input data is smaller than block size
        let root_hash = if tree.is_empty() {
            let mut data = Vec::new();
            input.read_to_end(&mut data)?;
            hash_one_block(&data, &salt, block_size, algorithm)?.as_ref().to_vec()
        } else {
            let first_block = &tree[0..block_size];
            hash_one_block(first_block, &salt, block_size, algorithm)?.as_ref().to_vec()
        };
        Ok(HashTree { tree, root_hash })
    }
}

/// Calculate hash tree for the blocks in `input`.
///
/// This function implements: https://www.kernel.org/doc/html/latest/filesystems/fsverity.html#merkle-tree
///
/// The file contents is divided into blocks, where the block size is configurable but is usually
/// 4096 bytes. The end of the last block is zero-padded if needed. Each block is then hashed,
/// producing the first level of hashes. Then, the hashes in this first level are grouped into
/// blocksize-byte blocks (zero-padding the ends as needed) and these blocks are hashed,
/// producing the second level of hashes. This proceeds up the tree until only a single block
/// remains.
pub fn generate_hash_tree<R: Read>(
    input: &mut R,
    input_size: usize,
    salt: &[u8],
    block_size: usize,
    algorithm: MessageDigest,
) -> Result<Vec<u8>> {
    let digest_size = algorithm.size();
    let levels = calc_hash_levels(input_size, block_size, digest_size);
    let tree_size = levels.iter().map(|r| r.len()).sum();

    // The contiguous memory that holds the entire merkle tree
    let mut hash_tree = vec![0; tree_size];

    for (n, cur) in levels.iter().enumerate() {
        if n == 0 {
            // Level 0: the (zero-padded) input stream is hashed into level 0
            let pad_size = round_to_multiple(input_size, block_size) - input_size;
            let mut input = input.chain(Cursor::new(vec![0; pad_size]));
            let mut level0 = Cursor::new(&mut hash_tree[cur.start..cur.end]);

            let mut a_block = vec![0; block_size];
            let mut num_blocks = input_size.div_ceil(block_size);
            while num_blocks > 0 {
                input.read_exact(&mut a_block)?;
                let h = hash_one_block(&a_block, salt, block_size, algorithm)?;
                level0.write_all(h.as_ref()).unwrap();
                num_blocks -= 1;
            }
        } else {
            // Intermediate levels: level n - 1 is hashed into level n
            // Both levels belong to the same `hash_tree`. In order to have a mutable slice for
            // level n while having a slice for level n - 1, take the mutable slice for both levels
            // and split it.
            let prev = &levels[n - 1];
            let cur_and_prev = &mut hash_tree[cur.start..prev.end];
            let (cur, prev) = cur_and_prev.split_at_mut(prev.start - cur.start);
            let mut cur = Cursor::new(cur);
            for data in prev.chunks(block_size) {
                let h = hash_one_block(data, salt, block_size, algorithm)?;
                cur.write_all(h.as_ref()).unwrap();
            }
        }
    }
    Ok(hash_tree)
}

/// Hash one block of input using the given hash algorithm and the salt. Input might be smaller
/// than a block, in which case zero is padded.
fn hash_one_block(
    input: &[u8],
    salt: &[u8],
    block_size: usize,
    algorithm: MessageDigest,
) -> Result<DigestBytes> {
    let mut ctx = Hasher::new(algorithm)?;
    ctx.update(salt)?;
    ctx.update(input)?;
    let pad_size = block_size - input.len();
    ctx.update(&vec![0; pad_size])?;
    Ok(ctx.finish()?)
}

type Range = std::ops::Range<usize>;

/// Calculate the ranges of hash for each level
fn calc_hash_levels(input_size: usize, block_size: usize, digest_size: usize) -> Vec<Range> {
    // The input is split into multiple blocks and each block is hashed, which becomes the input
    // for the next level. Size of a single hash is `digest_size`.
    let mut level_sizes = Vec::new();
    loop {
        // Input for this level is from either the last level (if exists), or the input parameter.
        let input_size = *level_sizes.last().unwrap_or(&input_size);
        if input_size <= block_size {
            break;
        }
        let num_blocks = input_size.div_ceil(block_size);
        let hashes_size = round_to_multiple(num_blocks * digest_size, block_size);
        level_sizes.push(hashes_size);
    }

    // The hash tree is stored upside down. The top level is at offset 0. The second level comes
    // next, and so on. Level 0 is located at the end.
    //
    // Given level_sizes [10, 3, 1], the offsets for each label are ...
    //
    // Level 2 is at offset 0
    // Level 1 is at offset 1 (because Level 2 is of size 1)
    // Level 0 is at offset 4 (because Level 1 is of size 3)
    //
    // This is done by scanning the sizes in reverse order
    let mut ranges = level_sizes
        .iter()
        .rev()
        .scan(0, |prev_end, size| {
            let range = *prev_end..*prev_end + size;
            *prev_end = range.end;
            Some(range)
        })
        .collect::<Vec<_>>();
    ranges.reverse(); // reverse again so that index N is for level N
    ranges
}

/// Round `n` up to the nearest multiple of `unit`
fn round_to_multiple(n: usize, unit: usize) -> usize {
    (n + unit - 1) & !(unit - 1)
}

/// Pad zero to salt if necessary.
///
/// According to https://www.kernel.org/doc/html/latest/filesystems/fsverity.html:
///
/// If a salt was specified, then it’s zero-padded to the closest multiple of the input size of the
/// hash algorithm’s compression function, e.g. 64 bytes for SHA-256 or 128 bytes for SHA-512. The
/// padded salt is prepended to every data or Merkle tree block that is hashed.
fn zero_pad_salt(salt: &[u8], algorithm: MessageDigest) -> Vec<u8> {
    if salt.is_empty() {
        salt.to_vec()
    } else {
        let padded_len = round_to_multiple(salt.len(), algorithm.block_size());
        let mut salt = salt.to_vec();
        salt.resize(padded_len, 0);
        salt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::hash::MessageDigest;
    use std::fs::{self, File};

    #[test]
    fn compare_with_golden_output() -> Result<()> {
        // The golden outputs are generated by using the `fsverity` utility.
        let sizes = ["512", "4K", "1M", "10000000", "272629760"];
        for size in sizes.iter() {
            let input_name = format!("tests/data/input.{}", size);
            let mut input = File::open(&input_name)?;
            let golden_hash_tree = fs::read(format!("{}.hash", input_name))?;
            let golden_descriptor = fs::read(format!("{}.descriptor", input_name))?;
            let golden_root_hash = &golden_descriptor[16..16 + 32];

            let size = std::fs::metadata(&input_name)?.len() as usize;
            let salt = vec![1, 2, 3, 4, 5, 6];
            let ht = HashTree::from(&mut input, size, &salt, 4096, MessageDigest::sha256())?;

            assert_eq!(golden_hash_tree.as_slice(), ht.tree.as_slice());
            assert_eq!(golden_root_hash, ht.root_hash.as_slice());
        }
        Ok(())
    }
}
