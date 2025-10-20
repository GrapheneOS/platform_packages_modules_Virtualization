// Copyright 2025, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Libraries for Linux /proc/*

use anyhow::{anyhow, Context, Error};
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

#[derive(Debug)]
pub struct ProcHelper {
    pid_to_comm: HashMap<u32, String>,
    inode_to_pid: HashMap<u64, u32>,
}

impl ProcHelper {
    fn get_socket_inode(fd: &Path) -> Option<u64> {
        let link = fs::read_link(fd).ok()?;
        let path_str = link.to_str()?;
        if path_str.starts_with("socket:[") {
            Some(path_str[8..path_str.len() - 1].parse().ok()?)
        } else {
            None
        }
    }

    // Iterate over /proc/*/fd to build inode lookup table while skipping files/dirs
    // without permissions.
    pub fn new() -> Result<Self, Error> {
        let mut pid_to_comm: HashMap<u32, String> = Default::default();
        let mut inode_to_pid: HashMap<u64, u32> = Default::default();

        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        };
        for comm in glob::glob_with("/proc/*/comm", options).unwrap() {
            let comm = comm?;
            let process = comm.parent().ok_or(anyhow!("Failed to read parent of {comm:?}"))?;
            let Ok(pid) = process
                .file_name()
                .ok_or(anyhow!("Failed to get name of {process:?}"))?
                .to_str()
                .ok_or(anyhow!("Failed to read name of {process:?}"))?
                .parse::<u32>()
            else {
                // Skip /proc/self/comm and /proc/thread-self/comm
                continue;
            };
            let comm = fs::read_to_string(&comm)
                .with_context(|| format!("Failed to read comm {comm:?}"))?
                .trim()
                .to_string();
            pid_to_comm.insert(pid, comm);

            let fd_dir = process.join("fd");
            let fds = match fs::read_dir(&fd_dir) {
                Ok(fds) => fds,
                Err(e) if e.kind() == ErrorKind::PermissionDenied => continue,
                e => e.with_context(|| format!("Failed to read dir {fd_dir:?}"))?,
            };
            for fd in fds {
                if let Some(inode) = Self::get_socket_inode(&fd?.path()) {
                    inode_to_pid.insert(inode, pid);
                }
            }
        }
        Ok(Self { pid_to_comm, inode_to_pid })
    }

    pub fn comm_with_inode(&self, inode: u64) -> Option<String> {
        let pid = self.inode_to_pid.get(&inode)?;
        self.pid_to_comm.get(pid).cloned()
    }
}
