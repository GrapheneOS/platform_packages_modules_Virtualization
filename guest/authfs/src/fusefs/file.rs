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

use log::error;
use std::convert::TryInto;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::file::{
    ChunkBuffer, EagerChunkReader, ReadByChunk, RemoteFileReader, RemoteMerkleTreeReader,
    VirtFdService,
};
use crate::fsverity::{merkle_tree_size, VerifiedFileReader};

enum FileInfo {
    ByPathUnderDirFd(i32, PathBuf),
    ByFd(i32),
}

type Reader = VerifiedFileReader<RemoteFileReader, EagerChunkReader>;

/// A lazily created read-only file that is verified against the given fs-verity digest.
///
/// The main purpose of this struct is to wrap and construct `VerifiedFileReader` lazily.
pub struct LazyVerifiedReadonlyFile {
    expected_digest: Vec<u8>,

    service: VirtFdService,
    file_info: FileInfo,

    /// A lazily instantiated reader.
    reader: Mutex<Option<Reader>>,
}

impl LazyVerifiedReadonlyFile {
    /// Prepare the file by a remote path, related to a remote directory FD.
    pub fn prepare_by_path(
        service: VirtFdService,
        remote_dir_fd: i32,
        remote_path: PathBuf,
        expected_digest: Vec<u8>,
    ) -> Self {
        LazyVerifiedReadonlyFile {
            service,
            file_info: FileInfo::ByPathUnderDirFd(remote_dir_fd, remote_path),
            expected_digest,
            reader: Mutex::new(None),
        }
    }

    /// Prepare the file by a remote file FD.
    pub fn prepare_by_fd(service: VirtFdService, remote_fd: i32, expected_digest: Vec<u8>) -> Self {
        LazyVerifiedReadonlyFile {
            service,
            file_info: FileInfo::ByFd(remote_fd),
            expected_digest,
            reader: Mutex::new(None),
        }
    }

    fn ensure_init_then<F, T>(&self, callback: F) -> io::Result<T>
    where
        F: FnOnce(&Reader) -> io::Result<T>,
    {
        let mut reader = self.reader.lock().unwrap();
        if reader.is_none() {
            let remote_file = match &self.file_info {
                FileInfo::ByPathUnderDirFd(dir_fd, related_path) => {
                    RemoteFileReader::new_by_path(self.service.clone(), *dir_fd, related_path)?
                }
                FileInfo::ByFd(file_fd) => RemoteFileReader::new(self.service.clone(), *file_fd),
            };
            let remote_fd = remote_file.get_remote_fd();
            let file_size = self
                .service
                .getFileSize(remote_fd)
                .map_err(|e| {
                    error!("Failed to get file size of remote fd {}: {}", remote_fd, e);
                    io::Error::from_raw_os_error(libc::EIO)
                })?
                .try_into()
                .map_err(|e| {
                    error!("Failed convert file size: {}", e);
                    io::Error::from_raw_os_error(libc::EIO)
                })?;
            let instance = VerifiedFileReader::new(
                remote_file,
                file_size,
                &self.expected_digest,
                EagerChunkReader::new(
                    RemoteMerkleTreeReader::new(self.service.clone(), remote_fd),
                    merkle_tree_size(file_size),
                )?,
            )
            .map_err(|e| {
                error!("Failed instantiate a verified file reader: {}", e);
                io::Error::from_raw_os_error(libc::EIO)
            })?;
            *reader = Some(instance);
        }
        callback(reader.as_ref().unwrap())
    }

    pub fn file_size(&self) -> io::Result<u64> {
        self.ensure_init_then(|reader| Ok(reader.file_size))
    }
}

impl ReadByChunk for LazyVerifiedReadonlyFile {
    fn read_chunk(&self, chunk_index: u64, buf: &mut ChunkBuffer) -> io::Result<usize> {
        self.ensure_init_then(|reader| reader.read_chunk(chunk_index, buf))
    }
}
