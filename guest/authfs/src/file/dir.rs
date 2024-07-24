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

use log::warn;
use nix::sys::stat::Mode;
use std::collections::{hash_map, HashMap};
use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use super::attr::Attr;
use super::remote_file::RemoteFileEditor;
use super::{validate_basename, VirtFdService, VirtFdServiceStatus};
use crate::fsverity::VerifiedFileEditor;
use crate::fusefs::{AuthFsDirEntry, Inode};

const MAX_ENTRIES: u16 = 1000; // Arbitrary limit

struct InodeInfo {
    inode: Inode,

    // This information is duplicated since it is also available in `AuthFs::inode_table` via the
    // type system. But it makes it simple to deal with deletion, where otherwise we need to get a
    // mutable parent directory in the table, and query the table for directory/file type checking
    // at the same time.
    is_dir: bool,
}

/// A remote directory backed by a remote directory FD, where the provider/fd_server is not
/// trusted.
///
/// The directory is assumed empty initially without the trust to the storage. Functionally, when
/// the backing storage is not clean, the fd_server can fail to create a file or directory when
/// there is name collision. From RemoteDirEditor's perspective of security, the creation failure
/// is just one of possible errors that can happen, and what matters is RemoteDirEditor maintains
/// the integrity itself.
///
/// When new files are created through RemoteDirEditor, the file integrity are maintained within the
/// VM. Similarly, integrity (namely the list of entries) of the directory, or new directories
/// created within such a directory, are also maintained within the VM. A compromised fd_server or
/// malicious client can't affect the view to the files and directories within such a directory in
/// the VM.
pub struct RemoteDirEditor {
    service: VirtFdService,
    remote_dir_fd: i32,

    /// Mapping of entry names to the corresponding inode. The actual file/directory is stored in
    /// the global pool in fusefs.
    entries: HashMap<PathBuf, InodeInfo>,
}

impl RemoteDirEditor {
    pub fn new(service: VirtFdService, remote_dir_fd: i32) -> Self {
        RemoteDirEditor { service, remote_dir_fd, entries: HashMap::new() }
    }

    /// Returns the number of entries created.
    pub fn number_of_entries(&self) -> u16 {
        self.entries.len() as u16 // limited to MAX_ENTRIES
    }

    /// Creates a remote file named `basename` with corresponding `inode` at the current directory.
    pub fn create_file(
        &mut self,
        basename: &Path,
        inode: Inode,
        mode: libc::mode_t,
    ) -> io::Result<(VerifiedFileEditor<RemoteFileEditor>, Attr)> {
        let mode = self.validate_arguments(basename, mode)?;
        let basename_str =
            basename.to_str().ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        let new_fd = self
            .service
            .createFileInDirectory(self.remote_dir_fd, basename_str, mode as i32)
            .map_err(into_io_error)?;

        let new_remote_file =
            VerifiedFileEditor::new(RemoteFileEditor::new(self.service.clone(), new_fd));
        self.entries.insert(basename.to_path_buf(), InodeInfo { inode, is_dir: false });
        let new_attr = Attr::new_file_with_mode(self.service.clone(), new_fd, mode);
        Ok((new_remote_file, new_attr))
    }

    /// Creates a remote directory named `basename` with corresponding `inode` at the current
    /// directory.
    pub fn mkdir(
        &mut self,
        basename: &Path,
        inode: Inode,
        mode: libc::mode_t,
    ) -> io::Result<(RemoteDirEditor, Attr)> {
        let mode = self.validate_arguments(basename, mode)?;
        let basename_str =
            basename.to_str().ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        let new_fd = self
            .service
            .createDirectoryInDirectory(self.remote_dir_fd, basename_str, mode as i32)
            .map_err(into_io_error)?;

        let new_remote_dir = RemoteDirEditor::new(self.service.clone(), new_fd);
        self.entries.insert(basename.to_path_buf(), InodeInfo { inode, is_dir: true });
        let new_attr = Attr::new_dir_with_mode(self.service.clone(), new_fd, mode);
        Ok((new_remote_dir, new_attr))
    }

    /// Deletes a file
    pub fn delete_file(&mut self, basename: &Path) -> io::Result<Inode> {
        let inode = self.force_delete_entry(basename, /* expect_dir */ false)?;

        let basename_str =
            basename.to_str().ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        if let Err(e) = self.service.deleteFile(self.remote_dir_fd, basename_str) {
            // Ignore the error to honor the local state.
            warn!("Deletion on the host is reportedly failed: {:?}", e);
        }
        Ok(inode)
    }

    /// Forces to delete a directory. The caller must only call if `basename` is a directory and
    /// empty.
    pub fn force_delete_directory(&mut self, basename: &Path) -> io::Result<Inode> {
        let inode = self.force_delete_entry(basename, /* expect_dir */ true)?;

        let basename_str =
            basename.to_str().ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        if let Err(e) = self.service.deleteDirectory(self.remote_dir_fd, basename_str) {
            // Ignore the error to honor the local state.
            warn!("Deletion on the host is reportedly failed: {:?}", e);
        }
        Ok(inode)
    }

    /// Returns the inode number of a file or directory named `name` previously created through
    /// `RemoteDirEditor`.
    pub fn find_inode(&self, name: &Path) -> io::Result<Inode> {
        self.entries
            .get(name)
            .map(|entry| entry.inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))
    }

    /// Returns whether the directory has an entry of the given name.
    pub fn has_entry(&self, name: &Path) -> bool {
        self.entries.contains_key(name)
    }

    pub fn retrieve_entries(&self) -> io::Result<Vec<AuthFsDirEntry>> {
        self.entries
            .iter()
            .map(|(name, InodeInfo { inode, is_dir })| {
                Ok(AuthFsDirEntry { inode: *inode, name: path_to_cstring(name)?, is_dir: *is_dir })
            })
            .collect::<io::Result<Vec<_>>>()
    }

    fn force_delete_entry(&mut self, basename: &Path, expect_dir: bool) -> io::Result<Inode> {
        // Kernel should only give us a basename.
        debug_assert!(validate_basename(basename).is_ok());

        if let Some(entry) = self.entries.get(basename) {
            match (expect_dir, entry.is_dir) {
                (true, false) => Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
                (false, true) => Err(io::Error::from_raw_os_error(libc::EISDIR)),
                _ => {
                    let inode = entry.inode;
                    let _ = self.entries.remove(basename);
                    Ok(inode)
                }
            }
        } else {
            Err(io::Error::from_raw_os_error(libc::ENOENT))
        }
    }

    fn validate_arguments(&self, basename: &Path, mode: u32) -> io::Result<u32> {
        // Kernel should only give us a basename.
        debug_assert!(validate_basename(basename).is_ok());

        if self.entries.contains_key(basename) {
            return Err(io::Error::from_raw_os_error(libc::EEXIST));
        }

        if self.entries.len() >= MAX_ENTRIES.into() {
            return Err(io::Error::from_raw_os_error(libc::EMLINK));
        }

        Ok(Mode::from_bits_truncate(mode).bits())
    }
}

/// An in-memory directory representation of a directory structure.
pub struct InMemoryDir(HashMap<PathBuf, InodeInfo>);

impl InMemoryDir {
    /// Creates an empty instance of `InMemoryDir`.
    pub fn new() -> Self {
        // Hash map is empty since "." and ".." are excluded in entries.
        InMemoryDir(HashMap::new())
    }

    /// Returns the number of entries in the directory (not including "." and "..").
    pub fn number_of_entries(&self) -> u16 {
        self.0.len() as u16 // limited to MAX_ENTRIES
    }

    /// Adds a directory name and its inode number to the directory. Fails if already exists. The
    /// caller is responsible for ensure the inode uniqueness.
    pub fn add_dir(&mut self, basename: &Path, inode: Inode) -> io::Result<()> {
        self.add_entry(basename, InodeInfo { inode, is_dir: true })
    }

    /// Adds a file name and its inode number to the directory. Fails if already exists. The
    /// caller is responsible for ensure the inode uniqueness.
    pub fn add_file(&mut self, basename: &Path, inode: Inode) -> io::Result<()> {
        self.add_entry(basename, InodeInfo { inode, is_dir: false })
    }

    fn add_entry(&mut self, basename: &Path, dir_entry: InodeInfo) -> io::Result<()> {
        validate_basename(basename)?;
        if self.0.len() >= MAX_ENTRIES.into() {
            return Err(io::Error::from_raw_os_error(libc::EMLINK));
        }

        if let hash_map::Entry::Vacant(entry) = self.0.entry(basename.to_path_buf()) {
            entry.insert(dir_entry);
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(libc::EEXIST))
        }
    }

    /// Looks up an entry inode by name. `None` if not found.
    pub fn lookup_inode(&self, basename: &Path) -> Option<Inode> {
        self.0.get(basename).map(|entry| entry.inode)
    }

    pub fn retrieve_entries(&self) -> io::Result<Vec<AuthFsDirEntry>> {
        self.0
            .iter()
            .map(|(name, InodeInfo { inode, is_dir })| {
                Ok(AuthFsDirEntry { inode: *inode, name: path_to_cstring(name)?, is_dir: *is_dir })
            })
            .collect::<io::Result<Vec<_>>>()
    }
}

fn path_to_cstring(path: &Path) -> io::Result<CString> {
    let bytes = OsString::from(path).into_vec();
    CString::new(bytes).map_err(|_| io::Error::from_raw_os_error(libc::EILSEQ))
}

fn into_io_error(e: VirtFdServiceStatus) -> io::Error {
    let maybe_errno = e.service_specific_error();
    if maybe_errno > 0 {
        io::Error::from_raw_os_error(maybe_errno)
    } else {
        io::Error::new(io::ErrorKind::Other, e.get_description())
    }
}
