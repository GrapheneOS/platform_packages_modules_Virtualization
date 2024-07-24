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
use nix::sys::stat::{mode_t, Mode, SFlag};
use std::io;

use super::VirtFdService;

/// Default/assumed mode of files not created by authfs.
///
/// For files that are given to authfs as FDs (i.e. not created through authfs), their mode is
/// unknown (or untrusted) until it is ever set. The default mode is just to make it
/// readable/writable to VFS. When the mode is set, the value on fd_server is supposed to become
/// consistent.
const DEFAULT_FILE_MODE: Mode =
    Mode::from_bits_truncate(Mode::S_IRUSR.bits() | Mode::S_IWUSR.bits());

/// Default/assumed mode of directories not created by authfs.
///
/// See above.
const DEFAULT_DIR_MODE: Mode = Mode::S_IRWXU;

/// `Attr` maintains the local truth for attributes (e.g. mode and type) while allowing setting the
/// remote attribute for the file description.
pub struct Attr {
    service: VirtFdService,
    mode: Mode,
    remote_fd: i32,
    is_dir: bool,
}

impl Attr {
    pub fn new_file(service: VirtFdService, remote_fd: i32) -> Attr {
        Attr { service, mode: DEFAULT_FILE_MODE, remote_fd, is_dir: false }
    }

    pub fn new_dir(service: VirtFdService, remote_fd: i32) -> Attr {
        Attr { service, mode: DEFAULT_DIR_MODE, remote_fd, is_dir: true }
    }

    pub fn new_file_with_mode(service: VirtFdService, remote_fd: i32, mode: mode_t) -> Attr {
        Attr { service, mode: Mode::from_bits_truncate(mode), remote_fd, is_dir: false }
    }

    pub fn new_dir_with_mode(service: VirtFdService, remote_fd: i32, mode: mode_t) -> Attr {
        Attr { service, mode: Mode::from_bits_truncate(mode), remote_fd, is_dir: true }
    }

    pub fn mode(&self) -> u32 {
        self.mode.bits()
    }

    /// Sets the file mode.
    ///
    /// In addition to the actual file mode, `encoded_mode` also contains information of the file
    /// type.
    pub fn set_mode(&mut self, encoded_mode: u32) -> io::Result<()> {
        let new_sflag = SFlag::from_bits_truncate(encoded_mode);
        let new_mode = Mode::from_bits_truncate(encoded_mode);

        let type_flag = if self.is_dir { SFlag::S_IFDIR } else { SFlag::S_IFREG };
        if !type_flag.contains(new_sflag) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }

        // Request for update only if changing.
        if new_mode != self.mode {
            self.service.chmod(self.remote_fd, new_mode.bits() as i32).map_err(|e| {
                error!(
                    "Failed to chmod (fd: {}, mode: {:o}) on fd_server: {:?}",
                    self.remote_fd, new_mode, e
                );
                io::Error::from_raw_os_error(libc::EIO)
            })?;
            self.mode = new_mode;
        }
        Ok(())
    }
}
