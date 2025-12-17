/*
 * Copyright (C) 2025 The Android Open Source Project
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
//! Commonly used and helpful utilities.
use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::os::fd::{OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::wrappers::compos_common_injection::odrefresh::is_system_property_interesting;
#[cfg(not(test))]
use crate::wrappers::compos_wrappers_injection::system_properties;

#[cfg(test)]
use crate::wrappers::compos_wrappers_injection::mock_system_properties as system_properties;

/// Returns an `OwnedFD` of the directory.
pub fn open_dir(path: &Path) -> Result<OwnedFd> {
    Ok(OwnedFd::from(
        OpenOptions::new()
            .custom_flags(libc::O_DIRECTORY)
            .read(true) // O_DIRECTORY can only be opened with read
            .open(path)
            .with_context(|| format!("Failed to open {path:?} directory as path fd"))?,
    ))
}

pub fn get_path_from_fd(fd: RawFd) -> PathBuf {
    match fs::read_link(format!("/proc/self/fd/{}", fd)) {
        Ok(fd_path) => fd_path.to_path_buf(),
        Err(e) => {
            eprintln!("Could not read link /proc/self/fd/{}:{}", fd, e);
            PathBuf::from("Unknown")
        }
    }
}

pub fn set_system_properties<F>(property_setter: F) -> Result<()>
where
    F: Fn(Vec<String>, Vec<String>) -> Result<()>,
{
    let mut names = Vec::new();
    let mut values = Vec::new();
    system_properties::foreach(|name, value| {
        if is_system_property_interesting(name) {
            names.push(name.to_owned());
            values.push(value.to_owned());
        }
    })?;
    property_setter(names, values)
}
