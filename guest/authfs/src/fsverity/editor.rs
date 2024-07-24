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

//! A module for writing to a file from a trusted world to an untrusted storage.
//!
//! Architectural Model:
//!  * Trusted world: the writer, a signing secret, has some memory, but NO persistent storage.
//!  * Untrusted world: persistent storage, assuming untrusted.
//!  * IPC mechanism between trusted and untrusted world
//!
//! Use cases:
//!  * In the trusted world, we want to generate a large file, sign it, and share the signature for
//!    a third party to verify the file.
//!  * In the trusted world, we want to read a previously signed file back with signature check
//!    without having to touch the whole file.
//!
//! Requirements:
//!  * Communication between trusted and untrusted world is not cheap, and files can be large.
//!  * A file write pattern may not be sequential, neither does read.
//!
//! Considering the above, a technique similar to fs-verity is used. fs-verity uses an alternative
//! hash function, a Merkle tree, to calculate the hash of file content. A file update at any
//! location will propagate the hash update from the leaf to the root node. Unlike fs-verity, which
//! assumes static files, to support write operation, we need to allow the file (thus tree) to
//! update.
//!
//! For the trusted world to generate a large file with random write and hash it, the writer needs
//! to hold some private information and update the Merkle tree during a file write (or even when
//! the Merkle tree needs to be stashed to the untrusted storage).
//!
//! A write to a file must update the root hash. In order for the root hash to update, a tree
//! walk to update from the write location to the root node is necessary. Importantly, in case when
//! (part of) the Merkle tree needs to be read from the untrusted storage (e.g. not yet verified in
//! cache), the original path must be verified by the trusted signature before the update to happen.
//!
//! Denial-of-service is a known weakness if the untrusted storage decides to simply remove the
//! file. But there is nothing we can do in this architecture.
//!
//! Rollback attack is another possible attack, but can be addressed with a rollback counter when
//! possible.

use std::io;
use std::sync::{Arc, RwLock};

use super::builder::MerkleLeaves;
use super::common::{Sha256Hash, SHA256_HASH_SIZE};
use crate::common::{ChunkedSizeIter, CHUNK_SIZE};
use crate::file::{ChunkBuffer, RandomWrite, ReadByChunk};
use openssl::sha::{sha256, Sha256};

fn debug_assert_usize_is_u64() {
    // Since we don't need to support 32-bit CPU, make an assert to make conversion between
    // u64 and usize easy below. Otherwise, we need to check `divide_roundup(offset + buf.len()
    // <= usize::MAX` or handle `TryInto` errors.
    debug_assert!(usize::MAX as u64 == u64::MAX, "Only 64-bit arch is supported");
}

/// VerifiedFileEditor provides an integrity layer to an underlying read-writable file, which may
/// not be stored in a trusted environment. Only new, empty files are currently supported.
pub struct VerifiedFileEditor<F: ReadByChunk + RandomWrite> {
    file: F,
    merkle_tree: Arc<RwLock<MerkleLeaves>>,
}

impl<F: ReadByChunk + RandomWrite> VerifiedFileEditor<F> {
    /// Wraps a supposedly new file for integrity protection.
    pub fn new(file: F) -> Self {
        Self { file, merkle_tree: Arc::new(RwLock::new(MerkleLeaves::new())) }
    }

    /// Returns the fs-verity digest size in bytes.
    pub fn get_fsverity_digest_size(&self) -> usize {
        SHA256_HASH_SIZE
    }

    /// Calculates the fs-verity digest of the current file.
    pub fn calculate_fsverity_digest(&self) -> io::Result<Sha256Hash> {
        let merkle_tree = self.merkle_tree.read().unwrap();
        merkle_tree.calculate_fsverity_digest().map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn read_backing_chunk_unverified(
        &self,
        chunk_index: u64,
        buf: &mut ChunkBuffer,
    ) -> io::Result<usize> {
        self.file.read_chunk(chunk_index, buf)
    }

    fn read_backing_chunk_verified(
        &self,
        chunk_index: u64,
        buf: &mut ChunkBuffer,
        merkle_tree_locked: &MerkleLeaves,
    ) -> io::Result<usize> {
        debug_assert_usize_is_u64();

        if merkle_tree_locked.is_index_valid(chunk_index as usize) {
            let size = self.read_backing_chunk_unverified(chunk_index, buf)?;

            // Ensure the returned buffer matches the known hash.
            let hash = sha256(buf);
            if !merkle_tree_locked.is_consistent(chunk_index as usize, &hash) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Inconsistent hash"));
            }
            Ok(size)
        } else {
            Ok(0)
        }
    }

    fn new_hash_for_incomplete_write(
        &self,
        source: &[u8],
        offset_from_alignment: usize,
        output_chunk_index: usize,
        merkle_tree: &mut MerkleLeaves,
    ) -> io::Result<Sha256Hash> {
        // The buffer is initialized to 0 purposely. To calculate the block hash, the data is
        // 0-padded to the block size. When a chunk read is less than a chunk, the initial value
        // conveniently serves the padding purpose.
        let mut orig_data = [0u8; CHUNK_SIZE as usize];

        // If previous data exists, read back and verify against the known hash (since the
        // storage / remote server is not trusted).
        if merkle_tree.is_index_valid(output_chunk_index) {
            self.read_backing_chunk_unverified(output_chunk_index as u64, &mut orig_data)?;

            // Verify original content
            let hash = sha256(&orig_data);
            if !merkle_tree.is_consistent(output_chunk_index, &hash) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Inconsistent hash"));
            }
        }

        let mut ctx = Sha256::new();
        ctx.update(&orig_data[..offset_from_alignment]);
        ctx.update(source);
        ctx.update(&orig_data[offset_from_alignment + source.len()..]);
        Ok(ctx.finish())
    }

    fn new_chunk_hash(
        &self,
        source: &[u8],
        offset_from_alignment: usize,
        current_size: usize,
        output_chunk_index: usize,
        merkle_tree: &mut MerkleLeaves,
    ) -> io::Result<Sha256Hash> {
        if current_size as u64 == CHUNK_SIZE {
            // Case 1: If the chunk is a complete one, just calculate the hash, regardless of
            // write location.
            Ok(sha256(source))
        } else {
            // Case 2: For an incomplete write, calculate the hash based on previous data (if
            // any).
            self.new_hash_for_incomplete_write(
                source,
                offset_from_alignment,
                output_chunk_index,
                merkle_tree,
            )
        }
    }

    pub fn size(&self) -> u64 {
        self.merkle_tree.read().unwrap().file_size()
    }
}

impl<F: ReadByChunk + RandomWrite> RandomWrite for VerifiedFileEditor<F> {
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        debug_assert_usize_is_u64();

        // The write range may not be well-aligned with the chunk boundary. There are various cases
        // to deal with:
        //  1. A write of a full 4K chunk.
        //  2. A write of an incomplete chunk, possibly beyond the original EOF.
        //
        // Note that a write beyond EOF can create a hole. But we don't need to handle it here
        // because holes are zeros, and leaves in MerkleLeaves are hashes of 4096-zeros by
        // default.

        // Now iterate on the input data, considering the alignment at the destination.
        for (output_offset, current_size) in
            ChunkedSizeIter::new(buf.len(), offset, CHUNK_SIZE as usize)
        {
            // Lock the tree for the whole write for now. There may be room to improve to increase
            // throughput.
            let mut merkle_tree = self.merkle_tree.write().unwrap();

            let offset_in_buf = (output_offset - offset) as usize;
            let source = &buf[offset_in_buf..offset_in_buf + current_size];
            let output_chunk_index = (output_offset / CHUNK_SIZE) as usize;
            let offset_from_alignment = (output_offset % CHUNK_SIZE) as usize;

            let new_hash = match self.new_chunk_hash(
                source,
                offset_from_alignment,
                current_size,
                output_chunk_index,
                &mut merkle_tree,
            ) {
                Ok(hash) => hash,
                Err(e) => {
                    // Return early when any error happens before the right. Even if the hash is not
                    // consistent for the current chunk, we can still consider the earlier writes
                    // successful. Note that nothing persistent has been done in this iteration.
                    let written = output_offset - offset;
                    if written > 0 {
                        return Ok(written as usize);
                    }
                    return Err(e);
                }
            };

            // A failed, partial write here will make the backing file inconsistent to the (old)
            // hash. Nothing can be done within this writer, but at least it still maintains the
            // (original) integrity for the file. To matches what write(2) describes for an error
            // case (though it's about direct I/O), "Partial data may be written ... should be
            // considered inconsistent", an error below is propagated.
            self.file.write_all_at(source, output_offset)?;

            // Update the hash only after the write succeeds. Note that this only attempts to keep
            // the tree consistent to what has been written regardless the actual state beyond the
            // writer.
            let size_at_least = offset.saturating_add(buf.len() as u64);
            merkle_tree.update_hash(output_chunk_index, &new_hash, size_at_least);
        }
        Ok(buf.len())
    }

    fn resize(&self, size: u64) -> io::Result<()> {
        debug_assert_usize_is_u64();

        let mut merkle_tree = self.merkle_tree.write().unwrap();
        // In case when we are truncating the file, we may need to recalculate the hash of the (new)
        // last chunk. Since the content is provided by the untrusted backend, we need to read the
        // data back first, verify it, then override the truncated portion with 0-padding for
        // hashing. As an optimization, we only need to read the data back if the new size isn't a
        // multiple of CHUNK_SIZE (since the hash is already correct).
        //
        // The same thing does not need to happen when the size is growing. Since the new extended
        // data is always 0, we can just resize the `MerkleLeaves`, where a new hash is always
        // calculated from 4096 zeros.
        if size < merkle_tree.file_size() && size % CHUNK_SIZE > 0 {
            let new_tail_size = (size % CHUNK_SIZE) as usize;
            let chunk_index = size / CHUNK_SIZE;
            if new_tail_size > 0 {
                let mut buf: ChunkBuffer = [0; CHUNK_SIZE as usize];
                let s = self.read_backing_chunk_verified(chunk_index, &mut buf, &merkle_tree)?;
                debug_assert!(new_tail_size <= s);

                let zeros = vec![0; CHUNK_SIZE as usize - new_tail_size];
                let mut ctx = Sha256::new();
                ctx.update(&buf[..new_tail_size]);
                ctx.update(&zeros);
                let new_hash = ctx.finish();
                merkle_tree.update_hash(chunk_index as usize, &new_hash, size);
            }
        }

        self.file.resize(size)?;
        merkle_tree.resize(size as usize);

        Ok(())
    }
}

impl<F: ReadByChunk + RandomWrite> ReadByChunk for VerifiedFileEditor<F> {
    fn read_chunk(&self, chunk_index: u64, buf: &mut ChunkBuffer) -> io::Result<usize> {
        let merkle_tree = self.merkle_tree.read().unwrap();
        self.read_backing_chunk_verified(chunk_index, buf, &merkle_tree)
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
    use std::cell::RefCell;
    use std::convert::TryInto;

    struct InMemoryEditor {
        data: RefCell<Vec<u8>>,
        fail_read: bool,
    }

    impl InMemoryEditor {
        pub fn new() -> InMemoryEditor {
            InMemoryEditor { data: RefCell::new(Vec::new()), fail_read: false }
        }
    }

    impl RandomWrite for InMemoryEditor {
        fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
            let begin: usize =
                offset.try_into().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            let end = begin + buf.len();
            if end > self.data.borrow().len() {
                self.data.borrow_mut().resize(end, 0);
            }
            self.data.borrow_mut().as_mut_slice()[begin..end].copy_from_slice(buf);
            Ok(buf.len())
        }

        fn resize(&self, size: u64) -> io::Result<()> {
            let size: usize =
                size.try_into().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            self.data.borrow_mut().resize(size, 0);
            Ok(())
        }
    }

    impl ReadByChunk for InMemoryEditor {
        fn read_chunk(&self, chunk_index: u64, buf: &mut ChunkBuffer) -> io::Result<usize> {
            if self.fail_read {
                return Err(io::Error::new(io::ErrorKind::Other, "test!"));
            }

            let borrowed = self.data.borrow();
            let chunk = &borrowed
                .chunks(CHUNK_SIZE as usize)
                .nth(chunk_index as usize)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("read_chunk out of bound: index {}", chunk_index),
                    )
                })?;
            buf[..chunk.len()].copy_from_slice(chunk);
            Ok(chunk.len())
        }
    }

    #[test]
    fn test_writer() -> Result<()> {
        let writer = InMemoryEditor::new();
        let buf = [1; 4096];
        assert_eq!(writer.data.borrow().len(), 0);

        assert_eq!(writer.write_at(&buf, 16384)?, 4096);
        assert_eq!(writer.data.borrow()[16384..16384 + 4096], buf);

        assert_eq!(writer.write_at(&buf, 2048)?, 4096);
        assert_eq!(writer.data.borrow()[2048..2048 + 4096], buf);

        assert_eq!(writer.data.borrow().len(), 16384 + 4096);
        Ok(())
    }

    #[test]
    fn test_verified_writer_no_write() -> Result<()> {
        // Verify fs-verity hash without any write.
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("3d248ca542a24fc62d1c43b916eae5016878e2533c88238480b26128a1f1af95")?
                .as_slice()
        );
        Ok(())
    }

    #[test]
    fn test_verified_writer_from_zero() -> Result<()> {
        // Verify a write of a full chunk.
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 4096], 0)?, 4096);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("cd0875ca59c7d37e962c5e8f5acd3770750ac80225e2df652ce5672fd34500af")?
                .as_slice()
        );

        // Verify a write of across multiple chunks.
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 4097], 0)?, 4097);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("2901b849fda2d91e3929524561c4a47e77bb64734319759507b2029f18b9cc52")?
                .as_slice()
        );

        // Verify another write of across multiple chunks.
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 10000], 0)?, 10000);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("7545409b556071554d18973a29b96409588c7cda4edd00d5586b27a11e1a523b")?
                .as_slice()
        );
        Ok(())
    }

    #[test]
    fn test_verified_writer_unaligned() -> Result<()> {
        // Verify small, unaligned write beyond EOF.
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 5], 3)?, 5);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("a23fc5130d3d7b3323fc4b4a5e79d5d3e9ddf3a3f5872639e867713512c6702f")?
                .as_slice()
        );

        // Verify bigger, unaligned write beyond EOF.
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 6000], 4000)?, 6000);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("d16d4c1c186d757e646f76208b21254f50d7f07ea07b1505ff48b2a6f603f989")?
                .as_slice()
        );
        Ok(())
    }

    #[test]
    fn test_verified_writer_with_hole() -> Result<()> {
        // Verify an aligned write beyond EOF with holes.
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 4096], 4096)?, 4096);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("4df2aefd8c2a9101d1d8770dca3ede418232eabce766bb8e020395eae2e97103")?
                .as_slice()
        );

        // Verify an unaligned write beyond EOF with holes.
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 5000], 6000)?, 5000);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("47d5da26f6934484e260630a69eb2eebb21b48f69bc8fbf8486d1694b7dba94f")?
                .as_slice()
        );

        // Just another example with a small write.
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 5], 16381)?, 5);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("8bd118821fb4aff26bb4b51d485cc481a093c68131b7f4f112e9546198449752")?
                .as_slice()
        );
        Ok(())
    }

    #[test]
    fn test_verified_writer_various_writes() -> Result<()> {
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 2048], 0)?, 2048);
        assert_eq!(file.write_at(&[1; 2048], 4096 + 2048)?, 2048);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("4c433d8640c888b629dc673d318cbb8d93b1eebcc784d9353e07f09f0dcfe707")?
                .as_slice()
        );
        assert_eq!(file.write_at(&[1; 2048], 2048)?, 2048);
        assert_eq!(file.write_at(&[1; 2048], 4096)?, 2048);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("2a476d58eb80394052a3a783111e1458ac3ecf68a7878183fed86ca0ff47ec0d")?
                .as_slice()
        );
        assert_eq!(file.write_at(&[0; 2048], 2048)?, 2048);
        assert_eq!(file.write_at(&[0; 2048], 4096)?, 2048);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("4c433d8640c888b629dc673d318cbb8d93b1eebcc784d9353e07f09f0dcfe707")?
                .as_slice()
        );
        assert_eq!(file.write_at(&[1; 4096], 2048)?, 4096);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("2a476d58eb80394052a3a783111e1458ac3ecf68a7878183fed86ca0ff47ec0d")?
                .as_slice()
        );
        assert_eq!(file.write_at(&[1; 2048], 8192)?, 2048);
        assert_eq!(file.write_at(&[1; 2048], 8192 + 2048)?, 2048);
        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("23cbac08371e6ee838ebcc7ae6512b939d2226e802337be7b383c3e046047d24")?
                .as_slice()
        );
        Ok(())
    }

    #[test]
    fn test_verified_writer_inconsistent_read() -> Result<()> {
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 8192], 0)?, 8192);

        // Replace the expected hash of the first/0-th chunk. An incomplete write will fail when it
        // detects the inconsistent read.
        {
            let mut merkle_tree = file.merkle_tree.write().unwrap();
            let overriding_hash = [42; SHA256_HASH_SIZE];
            merkle_tree.update_hash(0, &overriding_hash, 8192);
        }
        assert!(file.write_at(&[1; 1], 2048).is_err());

        // A write of full chunk can still succeed. Also fixed the inconsistency.
        assert_eq!(file.write_at(&[1; 4096], 4096)?, 4096);

        // Replace the expected hash of the second/1-th chunk. A write range from previous chunk can
        // still succeed, but returns early due to an inconsistent read but still successfully. A
        // resumed write will fail since no bytes can be written due to the same inconsistency.
        {
            let mut merkle_tree = file.merkle_tree.write().unwrap();
            let overriding_hash = [42; SHA256_HASH_SIZE];
            merkle_tree.update_hash(1, &overriding_hash, 8192);
        }
        assert_eq!(file.write_at(&[10; 8000], 0)?, 4096);
        assert!(file.write_at(&[10; 8000 - 4096], 4096).is_err());
        Ok(())
    }

    #[test]
    fn test_verified_writer_failed_read_back() -> Result<()> {
        let mut writer = InMemoryEditor::new();
        writer.fail_read = true;
        let file = VerifiedFileEditor::new(writer);
        assert_eq!(file.write_at(&[1; 8192], 0)?, 8192);

        // When a read back is needed, a read failure will fail to write.
        assert!(file.write_at(&[1; 1], 2048).is_err());
        Ok(())
    }

    #[test]
    fn test_resize_to_same_size() -> Result<()> {
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 2048], 0)?, 2048);

        assert!(file.resize(2048).is_ok());
        assert_eq!(file.size(), 2048);

        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("fef1b4f19bb7a2cd944d7cdee44d1accb12726389ca5b0f61ac0f548ae40876f")?
                .as_slice()
        );
        Ok(())
    }

    #[test]
    fn test_resize_to_grow() -> Result<()> {
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 2048], 0)?, 2048);

        // Resize should grow with 0s.
        assert!(file.resize(4096).is_ok());
        assert_eq!(file.size(), 4096);

        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("9e0e2745c21e4e74065240936d2047340d96a466680c3c9d177b82433e7a0bb1")?
                .as_slice()
        );
        Ok(())
    }

    #[test]
    fn test_resize_to_shrink() -> Result<()> {
        let file = VerifiedFileEditor::new(InMemoryEditor::new());
        assert_eq!(file.write_at(&[1; 4096], 0)?, 4096);

        // Truncate.
        file.resize(2048)?;
        assert_eq!(file.size(), 2048);

        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("fef1b4f19bb7a2cd944d7cdee44d1accb12726389ca5b0f61ac0f548ae40876f")?
                .as_slice()
        );
        Ok(())
    }

    #[test]
    fn test_resize_to_shrink_with_read_failure() -> Result<()> {
        let mut writer = InMemoryEditor::new();
        writer.fail_read = true;
        let file = VerifiedFileEditor::new(writer);
        assert_eq!(file.write_at(&[1; 4096], 0)?, 4096);

        // A truncate needs a read back. If the read fail, the resize should fail.
        assert!(file.resize(2048).is_err());
        Ok(())
    }

    #[test]
    fn test_resize_to_shirink_to_chunk_boundary() -> Result<()> {
        let mut writer = InMemoryEditor::new();
        writer.fail_read = true;
        let file = VerifiedFileEditor::new(writer);
        assert_eq!(file.write_at(&[1; 8192], 0)?, 8192);

        // Truncate to a chunk boundary. A read error doesn't matter since we won't need to
        // recalcuate the leaf hash.
        file.resize(4096)?;
        assert_eq!(file.size(), 4096);

        assert_eq!(
            file.calculate_fsverity_digest()?,
            hex::decode("cd0875ca59c7d37e962c5e8f5acd3770750ac80225e2df652ce5672fd34500af")?
                .as_slice()
        );
        Ok(())
    }
}
