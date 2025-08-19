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
//! A helper library for unit test.

// Test helpers for interrogating std::os::fd::RawFds.
pub mod fd {
    use std::path::PathBuf;
    use std::{fs, os::fd::RawFd};

    pub fn get_path(fd: &RawFd) -> PathBuf {
        match fs::read_link(format!("/proc/self/fd/{}", fd)) {
            Ok(fd_path) => fd_path.to_path_buf(),
            Err(e) => {
                eprintln!("Could not read link /proc/self/fd/{}:{}", fd, e);
                PathBuf::from("Unknown")
            }
        }
    }

    pub fn get_path_as_string(fd: &RawFd) -> String {
        get_path(fd).to_string_lossy().into_owned()
    }
}

// Test helpers for interrogating and manipulating ParcelFds.
pub mod parcel {
    use anyhow::{Context, Error};
    use binder::ParcelFileDescriptor as ParcelFd;
    use nix::fcntl::{fcntl, FcntlArg};
    use std::{fs::File, io::Write, os::fd::AsRawFd};

    pub fn write(parcel_fd: &ParcelFd, bytes: &[u8]) -> Result<(), Error> {
        parcel_fd
            .as_ref()
            .try_clone()
            .context("Failed to clone OwnedFd")
            .map(File::from)
            .and_then(|mut file| file.write_all(bytes).context("Failed to write to file"))
    }

    pub fn is_rw(parcel: &ParcelFd) -> bool {
        match fcntl(parcel.as_raw_fd(), FcntlArg::F_GETFL) {
            Err(e) => {
                eprintln!("F_GETFL failed: {}", e);
                false
            }
            Ok(flags) => flags & libc::O_RDWR != 0,
        }
    }
}

// Test helpers for interrogating and manipulating std::fs::File.
pub mod file {
    use super::fd;
    use anyhow::{Context, Error};
    use std::{
        fs,
        io::{Read, Seek, SeekFrom},
        os::fd::AsRawFd,
    };

    fn get_name(file: &fs::File) -> String {
        fd::get_path_as_string(&file.as_raw_fd())
    }

    fn as_vec(file: &mut fs::File) -> Result<Vec<u8>, Error> {
        let filename = get_name(file);
        let mut contents: Vec<u8> = Vec::new();
        file.seek(SeekFrom::Start(0)).with_context(|| format!("Rewinding {} failed", filename))?;
        file.read_to_end(&mut contents).with_context(|| format!("Reading {} failed", filename))?;
        Ok(contents)
    }

    pub fn contents_equals(file: &fs::File, content: &[u8]) -> bool {
        let filename = get_name(file);
        let file_copy = file.try_clone();
        if let Err(e) = file_copy {
            eprintln!("Failed to clone {}:{}", filename, e);
            return false;
        }
        match as_vec(&mut file_copy.unwrap()).map(|v| v == content) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("Failed to compare contents of {}:{}", filename, e);
                false
            }
        }
    }
}
