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

use log::error;
use std::convert::TryInto;
use std::io;

use crate::file::VirtFdService;
use authfs_aidl_interface::aidl::com::android::virt::fs::IVirtFdService::FsStat::FsStat;

/// Relevant/interesting stats of a remote filesystem.
pub struct RemoteFsStats {
    /// Block size of the filesystem
    pub block_size: u64,
    /// Fragment size of the filesystem
    pub fragment_size: u64,
    /// Number of blocks in the filesystem
    pub block_numbers: u64,
    /// Number of free blocks
    pub block_available: u64,
    /// Number of free inodes
    pub inodes_available: u64,
    /// Maximum filename length
    pub max_filename: u64,
}

pub struct RemoteFsStatsReader {
    service: VirtFdService,
}

impl RemoteFsStatsReader {
    pub fn new(service: VirtFdService) -> Self {
        Self { service }
    }

    pub fn statfs(&self) -> io::Result<RemoteFsStats> {
        let st = self.service.statfs().map_err(|e| {
            error!("Failed to call statfs on fd_server: {:?}", e);
            io::Error::from_raw_os_error(libc::EIO)
        })?;
        try_into_remote_fs_stats(st).map_err(|_| {
            error!("Received invalid stats from fd_server");
            io::Error::from_raw_os_error(libc::EIO)
        })
    }
}

fn try_into_remote_fs_stats(st: FsStat) -> Result<RemoteFsStats, std::num::TryFromIntError> {
    Ok(RemoteFsStats {
        block_size: st.blockSize.try_into()?,
        fragment_size: st.fragmentSize.try_into()?,
        block_numbers: st.blockNumbers.try_into()?,
        block_available: st.blockAvailable.try_into()?,
        inodes_available: st.inodesAvailable.try_into()?,
        max_filename: st.maxFilename.try_into()?,
    })
}
