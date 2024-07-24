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

use std::cmp::min;
use std::convert::TryFrom;
use std::io;
use std::path::Path;

use super::{ChunkBuffer, RandomWrite, ReadByChunk, VirtFdService};
use crate::common::CHUNK_SIZE;

fn remote_read_chunk(
    service: &VirtFdService,
    remote_fd: i32,
    chunk_index: u64,
    buf: &mut ChunkBuffer,
) -> io::Result<usize> {
    let offset = i64::try_from(chunk_index * CHUNK_SIZE)
        .map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))?;

    let chunk = service
        .readFile(remote_fd, offset, buf.len() as i32)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.get_description()))?;
    let size = min(buf.len(), chunk.len());
    buf[..size].copy_from_slice(&chunk[..size]);
    Ok(size)
}

pub struct RemoteFileReader {
    service: VirtFdService,
    file_fd: i32,
}

impl RemoteFileReader {
    pub fn new(service: VirtFdService, file_fd: i32) -> Self {
        RemoteFileReader { service, file_fd }
    }

    pub fn new_by_path(
        service: VirtFdService,
        dir_fd: i32,
        related_path: &Path,
    ) -> io::Result<Self> {
        let file_fd =
            service.openFileInDirectory(dir_fd, related_path.to_str().unwrap()).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "Failed to create a remote file reader by path {}: {}",
                        related_path.display(),
                        e.get_description()
                    ),
                )
            })?;
        Ok(RemoteFileReader { service, file_fd })
    }

    pub fn get_remote_fd(&self) -> i32 {
        self.file_fd
    }
}

impl ReadByChunk for RemoteFileReader {
    fn read_chunk(&self, chunk_index: u64, buf: &mut ChunkBuffer) -> io::Result<usize> {
        remote_read_chunk(&self.service, self.file_fd, chunk_index, buf)
    }
}

pub struct RemoteMerkleTreeReader {
    service: VirtFdService,
    file_fd: i32,
}

impl RemoteMerkleTreeReader {
    pub fn new(service: VirtFdService, file_fd: i32) -> Self {
        RemoteMerkleTreeReader { service, file_fd }
    }
}

impl ReadByChunk for RemoteMerkleTreeReader {
    fn read_chunk(&self, chunk_index: u64, buf: &mut ChunkBuffer) -> io::Result<usize> {
        let offset = i64::try_from(chunk_index * CHUNK_SIZE)
            .map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))?;

        let chunk = self
            .service
            .readFsverityMerkleTree(self.file_fd, offset, buf.len() as i32)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.get_description()))?;
        let size = min(buf.len(), chunk.len());
        buf[..size].copy_from_slice(&chunk[..size]);
        Ok(size)
    }
}

pub struct RemoteFileEditor {
    service: VirtFdService,
    file_fd: i32,
}

impl RemoteFileEditor {
    pub fn new(service: VirtFdService, file_fd: i32) -> Self {
        RemoteFileEditor { service, file_fd }
    }
}

impl RandomWrite for RemoteFileEditor {
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        let offset =
            i64::try_from(offset).map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
        let size = self
            .service
            .writeFile(self.file_fd, buf, offset)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.get_description()))?;
        Ok(size as usize) // within range because size is supposed to <= buf.len(), which is a usize
    }

    fn resize(&self, size: u64) -> io::Result<()> {
        let size =
            i64::try_from(size).map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
        self.service
            .resize(self.file_fd, size)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.get_description()))?;
        Ok(())
    }
}

impl ReadByChunk for RemoteFileEditor {
    fn read_chunk(&self, chunk_index: u64, buf: &mut ChunkBuffer) -> io::Result<usize> {
        remote_read_chunk(&self.service, self.file_fd, chunk_index, buf)
    }
}
