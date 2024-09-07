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

mod file;
mod mount;

use anyhow::{anyhow, bail, Result};
use fuse::filesystem::{
    Context, DirEntry, DirectoryIterator, Entry, FileSystem, FsOptions, GetxattrReply,
    SetattrValid, ZeroCopyReader, ZeroCopyWriter,
};
use fuse::sys::OpenOptions as FuseOpenOptions;
use log::{error, trace, warn};
use std::collections::{btree_map, BTreeMap};
use std::convert::{TryFrom, TryInto};
use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::mem::{zeroed, MaybeUninit};
use std::option::Option;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::common::{divide_roundup, ChunkedSizeIter, CHUNK_SIZE};
use crate::file::{
    validate_basename, Attr, InMemoryDir, RandomWrite, ReadByChunk, RemoteDirEditor,
    RemoteFileEditor, RemoteFileReader,
};
use crate::fsstat::RemoteFsStatsReader;
use crate::fsverity::VerifiedFileEditor;

pub use self::file::LazyVerifiedReadonlyFile;
pub use self::mount::mount_and_enter_message_loop;
use self::mount::MAX_WRITE_BYTES;

pub type Inode = u64;
type Handle = u64;

/// Maximum time for a file's metadata to be cached by the kernel. Since any file and directory
/// changes (if not read-only) has to go through AuthFS to be trusted, the timeout can be maximum.
const DEFAULT_METADATA_TIMEOUT: Duration = Duration::MAX;

const ROOT_INODE: Inode = 1;

/// `AuthFsEntry` defines the filesystem entry type supported by AuthFS.
pub enum AuthFsEntry {
    /// A read-only directory (writable during initialization). Root directory is an example.
    ReadonlyDirectory { dir: InMemoryDir },
    /// A file type that is verified against fs-verity signature (thus read-only). The file is
    /// served from a remote server.
    VerifiedReadonly { reader: LazyVerifiedReadonlyFile },
    /// A file type that is a read-only passthrough from a file on a remote server.
    UnverifiedReadonly { reader: RemoteFileReader, file_size: u64 },
    /// A file type that is initially empty, and the content is stored on a remote server. File
    /// integrity is guaranteed with private Merkle tree.
    VerifiedNew { editor: VerifiedFileEditor<RemoteFileEditor>, attr: Attr },
    /// A directory type that is initially empty. One can create new file (`VerifiedNew`) and new
    /// directory (`VerifiedNewDirectory` itself) with integrity guaranteed within the VM.
    VerifiedNewDirectory { dir: RemoteDirEditor, attr: Attr },
}

impl AuthFsEntry {
    fn expect_empty_deletable_directory(&self) -> io::Result<()> {
        match self {
            AuthFsEntry::VerifiedNewDirectory { dir, .. } => {
                if dir.number_of_entries() == 0 {
                    Ok(())
                } else {
                    Err(io::Error::from_raw_os_error(libc::ENOTEMPTY))
                }
            }
            AuthFsEntry::ReadonlyDirectory { .. } => {
                Err(io::Error::from_raw_os_error(libc::EACCES))
            }
            _ => Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        }
    }
}

struct InodeState {
    /// Actual inode entry.
    entry: AuthFsEntry,

    /// Number of `Handle`s (i.e. file descriptors) that are currently referring to the this inode.
    ///
    /// Technically, this does not matter to readonly entries, since they live forever. The
    /// reference count is only needed for manageing lifetime of writable entries like
    /// `VerifiedNew` and `VerifiedNewDirectory`. That is, when an entry is deleted, the actual
    /// entry needs to stay alive until the reference count reaches zero.
    ///
    /// Note: This is not to be confused with hardlinks, which AuthFS doesn't currently implement.
    handle_ref_count: AtomicU64,

    /// Whether the inode is already unlinked, i.e. should be removed, once `handle_ref_count` is
    /// down to zero.
    unlinked: bool,
}

impl InodeState {
    fn new(entry: AuthFsEntry) -> Self {
        InodeState { entry, handle_ref_count: AtomicU64::new(0), unlinked: false }
    }

    fn new_with_ref_count(entry: AuthFsEntry, handle_ref_count: u64) -> Self {
        InodeState { entry, handle_ref_count: AtomicU64::new(handle_ref_count), unlinked: false }
    }
}

/// Data type that a directory implementation should be able to present its entry to `AuthFs`.
#[derive(Clone)]
pub struct AuthFsDirEntry {
    pub inode: Inode,
    pub name: CString,
    pub is_dir: bool,
}

/// A snapshot of a directory entries for supporting `readdir` operation.
///
/// The `readdir` implementation is required by FUSE to not return any entries that have been
/// returned previously (while it's fine to not return new entries). Snapshot is the easiest way to
/// be compliant. See `fuse::filesystem::readdir` for more details.
///
/// A `DirEntriesSnapshot` is created on `opendir`, and is associated with the returned
/// `Handle`/FD. The snapshot is deleted when the handle is released in `releasedir`.
type DirEntriesSnapshot = Vec<AuthFsDirEntry>;

/// An iterator for reading from `DirEntriesSnapshot`.
pub struct DirEntriesSnapshotIterator {
    /// A reference to the `DirEntriesSnapshot` in `AuthFs`.
    snapshot: Arc<DirEntriesSnapshot>,

    /// A value determined by `Self` to identify the last entry. 0 is a reserved value by FUSE to
    /// mean reading from the beginning.
    prev_offset: usize,
}

impl DirectoryIterator for DirEntriesSnapshotIterator {
    fn next(&mut self) -> Option<DirEntry> {
        // This iterator should not be the only reference to the snapshot. The snapshot should
        // still be hold in `dir_handle_table`, i.e. when the FD is not yet closed.
        //
        // This code is unreachable when `readdir` is called with a closed FD. Only when the FD is
        // not yet closed, `DirEntriesSnapshotIterator` can be created (but still short-lived
        // during `readdir`).
        debug_assert!(Arc::strong_count(&self.snapshot) >= 2);

        // Since 0 is reserved, let's use 1-based index for the offset. This allows us to
        // resume from the previous read in the snapshot easily.
        let current_offset = if self.prev_offset == 0 {
            1 // first element in the vector
        } else {
            self.prev_offset + 1 // next element in the vector
        };
        if current_offset > self.snapshot.len() {
            None
        } else {
            let AuthFsDirEntry { inode, name, is_dir } = &self.snapshot[current_offset - 1];
            let entry = DirEntry {
                offset: current_offset as u64,
                ino: *inode,
                name,
                type_: if *is_dir { libc::DT_DIR.into() } else { libc::DT_REG.into() },
            };
            self.prev_offset = current_offset;
            Some(entry)
        }
    }
}

type DirHandleTable = BTreeMap<Handle, Arc<DirEntriesSnapshot>>;

// AuthFS needs to be `Sync` to be used with the `fuse` crate.
pub struct AuthFs {
    /// Table for `Inode` to `InodeState` lookup.
    inode_table: RwLock<BTreeMap<Inode, InodeState>>,

    /// The next available inode number.
    next_inode: AtomicU64,

    /// Table for `Handle` to `Arc<DirEntriesSnapshot>` lookup. On `opendir`, a new directory
    /// handle is created and the snapshot of the current directory is created. This is not
    /// super efficient, but is the simplest way to be compliant to the FUSE contract (see
    /// `fuse::filesystem::readdir`).
    ///
    /// Currently, no code locks `dir_handle_table` and `inode_table` at the same time to avoid
    /// deadlock.
    dir_handle_table: RwLock<DirHandleTable>,

    /// The next available handle number.
    next_handle: AtomicU64,

    /// A reader to access the remote filesystem stats, which is supposed to be of "the" output
    /// directory. We assume all output are stored in the same partition.
    remote_fs_stats_reader: RemoteFsStatsReader,
}

// Implementation for preparing an `AuthFs` instance, before starting to serve.
// TODO(victorhsieh): Consider implement a builder to separate the mutable initialization from the
// immutable / interiorly mutable serving phase.
impl AuthFs {
    pub fn new(remote_fs_stats_reader: RemoteFsStatsReader) -> AuthFs {
        let mut inode_table = BTreeMap::new();
        inode_table.insert(
            ROOT_INODE,
            InodeState::new(AuthFsEntry::ReadonlyDirectory { dir: InMemoryDir::new() }),
        );

        AuthFs {
            inode_table: RwLock::new(inode_table),
            next_inode: AtomicU64::new(ROOT_INODE + 1),
            dir_handle_table: RwLock::new(BTreeMap::new()),
            next_handle: AtomicU64::new(1),
            remote_fs_stats_reader,
        }
    }

    /// Add an `AuthFsEntry` as `basename` to the filesystem root.
    pub fn add_entry_at_root_dir(
        &mut self,
        basename: PathBuf,
        entry: AuthFsEntry,
    ) -> Result<Inode> {
        validate_basename(&basename)?;
        self.add_entry_at_ro_dir_by_path(ROOT_INODE, &basename, entry)
    }

    /// Add an `AuthFsEntry` by path from the `ReadonlyDirectory` represented by `dir_inode`. The
    /// path must be a related path. If some ancestor directories do not exist, they will be
    /// created (also as `ReadonlyDirectory`) automatically.
    pub fn add_entry_at_ro_dir_by_path(
        &mut self,
        dir_inode: Inode,
        path: &Path,
        entry: AuthFsEntry,
    ) -> Result<Inode> {
        // 1. Make sure the parent directories all exist. Derive the entry's parent inode.
        let parent_path =
            path.parent().ok_or_else(|| anyhow!("No parent directory: {:?}", path))?;
        let parent_inode =
            parent_path.components().try_fold(dir_inode, |current_dir_inode, path_component| {
                match path_component {
                    Component::RootDir => bail!("Absolute path is not supported"),
                    Component::Normal(name) => {
                        let inode_table = self.inode_table.get_mut().unwrap();
                        // Locate the internal directory structure.
                        let current_dir_entry = &mut inode_table
                            .get_mut(&current_dir_inode)
                            .ok_or_else(|| {
                                anyhow!("Unknown directory inode {}", current_dir_inode)
                            })?
                            .entry;
                        let dir = match current_dir_entry {
                            AuthFsEntry::ReadonlyDirectory { dir } => dir,
                            _ => unreachable!("Not a ReadonlyDirectory"),
                        };
                        // Return directory inode. Create first if not exists.
                        if let Some(existing_inode) = dir.lookup_inode(name.as_ref()) {
                            Ok(existing_inode)
                        } else {
                            let new_inode = self.next_inode.fetch_add(1, Ordering::Relaxed);
                            let new_dir_entry =
                                AuthFsEntry::ReadonlyDirectory { dir: InMemoryDir::new() };

                            // Actually update the tables.
                            dir.add_dir(name.as_ref(), new_inode)?;
                            if inode_table
                                .insert(new_inode, InodeState::new(new_dir_entry))
                                .is_some()
                            {
                                bail!("Unexpected to find a duplicated inode");
                            }
                            Ok(new_inode)
                        }
                    }
                    _ => Err(anyhow!("Path is not canonical: {:?}", path)),
                }
            })?;

        // 2. Insert the entry to the parent directory, as well as the inode table.
        let inode_table = self.inode_table.get_mut().unwrap();
        let inode_state = inode_table.get_mut(&parent_inode).expect("previously returned inode");
        match &mut inode_state.entry {
            AuthFsEntry::ReadonlyDirectory { dir } => {
                let basename =
                    path.file_name().ok_or_else(|| anyhow!("Bad file name: {:?}", path))?;
                let new_inode = self.next_inode.fetch_add(1, Ordering::Relaxed);

                // Actually update the tables.
                dir.add_file(basename.as_ref(), new_inode)?;
                if inode_table.insert(new_inode, InodeState::new(entry)).is_some() {
                    bail!("Unexpected to find a duplicated inode");
                }
                Ok(new_inode)
            }
            _ => unreachable!("Not a ReadonlyDirectory"),
        }
    }
}

// Implementation for serving requests.
impl AuthFs {
    /// Handles the file associated with `inode` if found. This function returns whatever
    /// `handle_fn` returns.
    fn handle_inode<F, R>(&self, inode: &Inode, handle_fn: F) -> io::Result<R>
    where
        F: FnOnce(&AuthFsEntry) -> io::Result<R>,
    {
        let inode_table = self.inode_table.read().unwrap();
        handle_inode_locked(&inode_table, inode, |inode_state| handle_fn(&inode_state.entry))
    }

    /// Adds a new entry `name` created by `create_fn` at `parent_inode`, with an initial ref count
    /// of one.
    ///
    /// The operation involves two updates: adding the name with a new allocated inode to the
    /// parent directory, and insert the new inode and the actual `AuthFsEntry` to the global inode
    /// table.
    ///
    /// `create_fn` receives the parent directory, through which it can create the new entry at and
    /// register the new inode to. Its returned entry is then added to the inode table.
    fn create_new_entry_with_ref_count<F>(
        &self,
        parent_inode: Inode,
        name: &CStr,
        create_fn: F,
    ) -> io::Result<Inode>
    where
        F: FnOnce(&mut AuthFsEntry, &Path, Inode) -> io::Result<AuthFsEntry>,
    {
        let mut inode_table = self.inode_table.write().unwrap();
        let (new_inode, new_file_entry) = handle_inode_mut_locked(
            &mut inode_table,
            &parent_inode,
            |InodeState { entry, .. }| {
                let new_inode = self.next_inode.fetch_add(1, Ordering::Relaxed);
                let basename: &Path = cstr_to_path(name);
                let new_file_entry = create_fn(entry, basename, new_inode)?;
                Ok((new_inode, new_file_entry))
            },
        )?;

        if let btree_map::Entry::Vacant(entry) = inode_table.entry(new_inode) {
            entry.insert(InodeState::new_with_ref_count(new_file_entry, 1));
            Ok(new_inode)
        } else {
            unreachable!("Unexpected duplication of inode {}", new_inode);
        }
    }

    fn open_dir_store_snapshot(
        &self,
        dir_entries: Vec<AuthFsDirEntry>,
    ) -> io::Result<(Option<Handle>, FuseOpenOptions)> {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        let mut dir_handle_table = self.dir_handle_table.write().unwrap();
        if let btree_map::Entry::Vacant(value) = dir_handle_table.entry(handle) {
            value.insert(Arc::new(dir_entries));
            Ok((Some(handle), FuseOpenOptions::empty()))
        } else {
            unreachable!("Unexpected to see new handle {} to existing in the table", handle);
        }
    }
}

fn check_access_mode(flags: u32, mode: libc::c_int) -> io::Result<()> {
    if (flags & libc::O_ACCMODE as u32) == mode as u32 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(libc::EACCES))
    }
}

#[allow(clippy::enum_variant_names)]
enum AccessMode {
    ReadOnly,
    Variable(u32),
}

fn create_stat(
    ino: libc::ino_t,
    file_size: u64,
    access_mode: AccessMode,
) -> io::Result<libc::stat64> {
    // SAFETY: stat64 is a plan C struct without pointer.
    let mut st = unsafe { MaybeUninit::<libc::stat64>::zeroed().assume_init() };

    st.st_ino = ino;
    st.st_mode = match access_mode {
        AccessMode::ReadOnly => {
            // Until needed, let's just grant the owner access.
            libc::S_IFREG | libc::S_IRUSR
        }
        AccessMode::Variable(mode) => libc::S_IFREG | mode,
    };
    st.st_nlink = 1;
    st.st_uid = 0;
    st.st_gid = 0;
    st.st_size = libc::off64_t::try_from(file_size)
        .map_err(|_| io::Error::from_raw_os_error(libc::EFBIG))?;
    st.st_blksize = CHUNK_SIZE.try_into().unwrap();
    // Per man stat(2), st_blocks is "Number of 512B blocks allocated".
    st.st_blocks = libc::c_longlong::try_from(divide_roundup(file_size, 512))
        .map_err(|_| io::Error::from_raw_os_error(libc::EFBIG))?;
    Ok(st)
}

fn create_dir_stat(
    ino: libc::ino_t,
    file_number: u16,
    access_mode: AccessMode,
) -> io::Result<libc::stat64> {
    // SAFETY: stat64 is a plan C struct without pointer.
    let mut st = unsafe { MaybeUninit::<libc::stat64>::zeroed().assume_init() };

    st.st_ino = ino;
    st.st_mode = match access_mode {
        AccessMode::ReadOnly => {
            // Until needed, let's just grant the owner access and search to group and others.
            libc::S_IFDIR | libc::S_IXUSR | libc::S_IRUSR | libc::S_IXGRP | libc::S_IXOTH
        }
        AccessMode::Variable(mode) => libc::S_IFDIR | mode,
    };

    // 2 extra for . and ..
    st.st_nlink = file_number
        .checked_add(2)
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?
        .into();

    st.st_uid = 0;
    st.st_gid = 0;
    Ok(st)
}

fn offset_to_chunk_index(offset: u64) -> u64 {
    offset / CHUNK_SIZE
}

fn read_chunks<W: io::Write, T: ReadByChunk>(
    mut w: W,
    file: &T,
    file_size: u64,
    offset: u64,
    size: u32,
) -> io::Result<usize> {
    let remaining = file_size.saturating_sub(offset);
    let size_to_read = std::cmp::min(size as usize, remaining as usize);
    let total = ChunkedSizeIter::new(size_to_read, offset, CHUNK_SIZE as usize).try_fold(
        0,
        |total, (current_offset, planned_data_size)| {
            // TODO(victorhsieh): There might be a non-trivial way to avoid this copy. For example,
            // instead of accepting a buffer, the writer could expose the final destination buffer
            // for the reader to write to. It might not be generally applicable though, e.g. with
            // virtio transport, the buffer may not be continuous.
            let mut buf = [0u8; CHUNK_SIZE as usize];
            let read_size = file.read_chunk(offset_to_chunk_index(current_offset), &mut buf)?;
            if read_size < planned_data_size {
                return Err(io::Error::from_raw_os_error(libc::ENODATA));
            }

            let begin = (current_offset % CHUNK_SIZE) as usize;
            let end = begin + planned_data_size;
            let s = w.write(&buf[begin..end])?;
            if s != planned_data_size {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            Ok(total + s)
        },
    )?;

    Ok(total)
}

impl FileSystem for AuthFs {
    type Inode = Inode;
    type Handle = Handle;
    type DirIter = DirEntriesSnapshotIterator;

    fn max_buffer_size(&self) -> u32 {
        MAX_WRITE_BYTES
    }

    fn init(&self, _capable: FsOptions) -> io::Result<FsOptions> {
        // Enable writeback cache for better performance especially since our bandwidth to the
        // backend service is limited.
        Ok(FsOptions::WRITEBACK_CACHE)
    }

    fn lookup(&self, _ctx: Context, parent: Inode, name: &CStr) -> io::Result<Entry> {
        let inode_table = self.inode_table.read().unwrap();

        // Look up the entry's inode number in parent directory.
        let inode =
            handle_inode_locked(&inode_table, &parent, |inode_state| match &inode_state.entry {
                AuthFsEntry::ReadonlyDirectory { dir } => {
                    let path = cstr_to_path(name);
                    dir.lookup_inode(path).ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))
                }
                AuthFsEntry::VerifiedNewDirectory { dir, .. } => {
                    let path = cstr_to_path(name);
                    dir.find_inode(path)
                }
                _ => Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
            })?;

        // Create the entry's stat if found.
        let st = handle_inode_locked(
            &inode_table,
            &inode,
            |InodeState { entry, handle_ref_count, .. }| {
                let st = match entry {
                    AuthFsEntry::ReadonlyDirectory { dir } => {
                        create_dir_stat(inode, dir.number_of_entries(), AccessMode::ReadOnly)
                    }
                    AuthFsEntry::UnverifiedReadonly { file_size, .. } => {
                        create_stat(inode, *file_size, AccessMode::ReadOnly)
                    }
                    AuthFsEntry::VerifiedReadonly { reader } => {
                        create_stat(inode, reader.file_size()?, AccessMode::ReadOnly)
                    }
                    AuthFsEntry::VerifiedNew { editor, attr, .. } => {
                        create_stat(inode, editor.size(), AccessMode::Variable(attr.mode()))
                    }
                    AuthFsEntry::VerifiedNewDirectory { dir, attr } => create_dir_stat(
                        inode,
                        dir.number_of_entries(),
                        AccessMode::Variable(attr.mode()),
                    ),
                }?;
                if handle_ref_count.fetch_add(1, Ordering::Relaxed) == u64::MAX {
                    panic!("Handle reference count overflow");
                }
                Ok(st)
            },
        )?;

        Ok(Entry {
            inode,
            generation: 0,
            attr: st,
            entry_timeout: DEFAULT_METADATA_TIMEOUT,
            attr_timeout: DEFAULT_METADATA_TIMEOUT,
        })
    }

    fn forget(&self, _ctx: Context, inode: Self::Inode, count: u64) {
        let mut inode_table = self.inode_table.write().unwrap();
        let delete_now = handle_inode_mut_locked(
            &mut inode_table,
            &inode,
            |InodeState { handle_ref_count, unlinked, .. }| {
                let current = handle_ref_count.get_mut();
                if count > *current {
                    error!(
                        "Trying to decrease refcount of inode {} by {} (> current {})",
                        inode, count, *current
                    );
                    panic!(); // log to logcat with error!
                }
                *current -= count;
                Ok(*unlinked && *current == 0)
            },
        );

        match delete_now {
            Ok(true) => {
                let _ignored = inode_table.remove(&inode).expect("Removed an existing entry");
            }
            Ok(false) => { /* Let the inode stay */ }
            Err(e) => {
                warn!(
                    "Unexpected failure when tries to forget an inode {} by refcount {}: {:?}",
                    inode, count, e
                );
            }
        }
    }

    fn getattr(
        &self,
        _ctx: Context,
        inode: Inode,
        _handle: Option<Handle>,
    ) -> io::Result<(libc::stat64, Duration)> {
        self.handle_inode(&inode, |config| {
            Ok((
                match config {
                    AuthFsEntry::ReadonlyDirectory { dir } => {
                        create_dir_stat(inode, dir.number_of_entries(), AccessMode::ReadOnly)
                    }
                    AuthFsEntry::UnverifiedReadonly { file_size, .. } => {
                        create_stat(inode, *file_size, AccessMode::ReadOnly)
                    }
                    AuthFsEntry::VerifiedReadonly { reader } => {
                        create_stat(inode, reader.file_size()?, AccessMode::ReadOnly)
                    }
                    AuthFsEntry::VerifiedNew { editor, attr, .. } => {
                        create_stat(inode, editor.size(), AccessMode::Variable(attr.mode()))
                    }
                    AuthFsEntry::VerifiedNewDirectory { dir, attr } => create_dir_stat(
                        inode,
                        dir.number_of_entries(),
                        AccessMode::Variable(attr.mode()),
                    ),
                }?,
                DEFAULT_METADATA_TIMEOUT,
            ))
        })
    }

    fn open(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        flags: u32,
    ) -> io::Result<(Option<Self::Handle>, FuseOpenOptions)> {
        // Since file handle is not really used in later operations (which use Inode directly),
        // return None as the handle.
        self.handle_inode(&inode, |config| {
            match config {
                AuthFsEntry::VerifiedReadonly { .. } | AuthFsEntry::UnverifiedReadonly { .. } => {
                    check_access_mode(flags, libc::O_RDONLY)?;
                }
                AuthFsEntry::VerifiedNew { .. } => {
                    // TODO(victorhsieh): Imeplement ACL check using the attr and ctx. Always allow
                    // for now.
                }
                AuthFsEntry::ReadonlyDirectory { .. }
                | AuthFsEntry::VerifiedNewDirectory { .. } => {
                    // TODO(victorhsieh): implement when needed.
                    return Err(io::Error::from_raw_os_error(libc::ENOSYS));
                }
            }
            // Always cache the file content. There is currently no need to support direct I/O or
            // avoid the cache buffer. Memory mapping is only possible with cache enabled.
            Ok((None, FuseOpenOptions::KEEP_CACHE))
        })
    }

    fn create(
        &self,
        _ctx: Context,
        parent: Self::Inode,
        name: &CStr,
        mode: u32,
        _flags: u32,
        umask: u32,
        _security_ctx: Option<&CStr>,
    ) -> io::Result<(Entry, Option<Self::Handle>, FuseOpenOptions)> {
        let new_inode = self.create_new_entry_with_ref_count(
            parent,
            name,
            |parent_entry, basename, new_inode| match parent_entry {
                AuthFsEntry::VerifiedNewDirectory { dir, .. } => {
                    if dir.has_entry(basename) {
                        return Err(io::Error::from_raw_os_error(libc::EEXIST));
                    }
                    let mode = mode & !umask;
                    let (new_file, new_attr) = dir.create_file(basename, new_inode, mode)?;
                    Ok(AuthFsEntry::VerifiedNew { editor: new_file, attr: new_attr })
                }
                _ => Err(io::Error::from_raw_os_error(libc::EBADF)),
            },
        )?;

        Ok((
            Entry {
                inode: new_inode,
                generation: 0,
                attr: create_stat(new_inode, /* file_size */ 0, AccessMode::Variable(mode))?,
                entry_timeout: DEFAULT_METADATA_TIMEOUT,
                attr_timeout: DEFAULT_METADATA_TIMEOUT,
            },
            // See also `open`.
            /* handle */ None,
            FuseOpenOptions::KEEP_CACHE,
        ))
    }

    fn read<W: io::Write + ZeroCopyWriter>(
        &self,
        _ctx: Context,
        inode: Inode,
        _handle: Handle,
        w: W,
        size: u32,
        offset: u64,
        _lock_owner: Option<u64>,
        _flags: u32,
    ) -> io::Result<usize> {
        self.handle_inode(&inode, |config| {
            match config {
                AuthFsEntry::VerifiedReadonly { reader } => {
                    read_chunks(w, reader, reader.file_size()?, offset, size)
                }
                AuthFsEntry::UnverifiedReadonly { reader, file_size } => {
                    read_chunks(w, reader, *file_size, offset, size)
                }
                AuthFsEntry::VerifiedNew { editor, .. } => {
                    // Note that with FsOptions::WRITEBACK_CACHE, it's possible for the kernel to
                    // request a read even if the file is open with O_WRONLY.
                    read_chunks(w, editor, editor.size(), offset, size)
                }
                AuthFsEntry::ReadonlyDirectory { .. }
                | AuthFsEntry::VerifiedNewDirectory { .. } => {
                    Err(io::Error::from_raw_os_error(libc::EISDIR))
                }
            }
        })
    }

    fn write<R: io::Read + ZeroCopyReader>(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _handle: Self::Handle,
        mut r: R,
        size: u32,
        offset: u64,
        _lock_owner: Option<u64>,
        _delayed_write: bool,
        _flags: u32,
    ) -> io::Result<usize> {
        self.handle_inode(&inode, |config| match config {
            AuthFsEntry::VerifiedNew { editor, .. } => {
                let mut buf = vec![0; size as usize];
                r.read_exact(&mut buf)?;
                editor.write_at(&buf, offset)
            }
            AuthFsEntry::VerifiedReadonly { .. } | AuthFsEntry::UnverifiedReadonly { .. } => {
                Err(io::Error::from_raw_os_error(libc::EPERM))
            }
            AuthFsEntry::ReadonlyDirectory { .. } | AuthFsEntry::VerifiedNewDirectory { .. } => {
                Err(io::Error::from_raw_os_error(libc::EISDIR))
            }
        })
    }

    fn setattr(
        &self,
        _ctx: Context,
        inode: Inode,
        in_attr: libc::stat64,
        _handle: Option<Handle>,
        valid: SetattrValid,
    ) -> io::Result<(libc::stat64, Duration)> {
        let mut inode_table = self.inode_table.write().unwrap();
        handle_inode_mut_locked(&mut inode_table, &inode, |InodeState { entry, .. }| match entry {
            AuthFsEntry::VerifiedNew { editor, attr } => {
                check_unsupported_setattr_request(valid)?;

                // Initialize the default stat.
                let mut new_attr =
                    create_stat(inode, editor.size(), AccessMode::Variable(attr.mode()))?;
                // `valid` indicates what fields in `attr` are valid. Update to return correctly.
                if valid.contains(SetattrValid::SIZE) {
                    // st_size is i64, but the cast should be safe since kernel should not give a
                    // negative size.
                    debug_assert!(in_attr.st_size >= 0);
                    new_attr.st_size = in_attr.st_size;
                    editor.resize(in_attr.st_size as u64)?;
                }
                if valid.contains(SetattrValid::MODE) {
                    attr.set_mode(in_attr.st_mode)?;
                    new_attr.st_mode = in_attr.st_mode;
                }
                Ok((new_attr, DEFAULT_METADATA_TIMEOUT))
            }
            AuthFsEntry::VerifiedNewDirectory { dir, attr } => {
                check_unsupported_setattr_request(valid)?;
                if valid.contains(SetattrValid::SIZE) {
                    return Err(io::Error::from_raw_os_error(libc::EISDIR));
                }

                // Initialize the default stat.
                let mut new_attr = create_dir_stat(
                    inode,
                    dir.number_of_entries(),
                    AccessMode::Variable(attr.mode()),
                )?;
                if valid.contains(SetattrValid::MODE) {
                    attr.set_mode(in_attr.st_mode)?;
                    new_attr.st_mode = in_attr.st_mode;
                }
                Ok((new_attr, DEFAULT_METADATA_TIMEOUT))
            }
            _ => Err(io::Error::from_raw_os_error(libc::EPERM)),
        })
    }

    fn getxattr(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        name: &CStr,
        size: u32,
    ) -> io::Result<GetxattrReply> {
        self.handle_inode(&inode, |config| {
            match config {
                AuthFsEntry::VerifiedNew { editor, .. } => {
                    // FUSE ioctl is limited, thus we can't implement fs-verity ioctls without a
                    // kernel change (see b/196635431). Until it's possible, use
                    // xattr to expose what we need as an authfs specific API.
                    if name != CStr::from_bytes_with_nul(b"authfs.fsverity.digest\0").unwrap() {
                        return Err(io::Error::from_raw_os_error(libc::ENODATA));
                    }

                    if size == 0 {
                        // Per protocol, when size is 0, return the value size.
                        Ok(GetxattrReply::Count(editor.get_fsverity_digest_size() as u32))
                    } else {
                        let digest = editor.calculate_fsverity_digest()?;
                        if digest.len() > size as usize {
                            Err(io::Error::from_raw_os_error(libc::ERANGE))
                        } else {
                            Ok(GetxattrReply::Value(digest.to_vec()))
                        }
                    }
                }
                _ => Err(io::Error::from_raw_os_error(libc::ENODATA)),
            }
        })
    }

    fn mkdir(
        &self,
        _ctx: Context,
        parent: Self::Inode,
        name: &CStr,
        mode: u32,
        umask: u32,
        _security_ctx: Option<&CStr>,
    ) -> io::Result<Entry> {
        let new_inode = self.create_new_entry_with_ref_count(
            parent,
            name,
            |parent_entry, basename, new_inode| match parent_entry {
                AuthFsEntry::VerifiedNewDirectory { dir, .. } => {
                    if dir.has_entry(basename) {
                        return Err(io::Error::from_raw_os_error(libc::EEXIST));
                    }
                    let mode = mode & !umask;
                    let (new_dir, new_attr) = dir.mkdir(basename, new_inode, mode)?;
                    Ok(AuthFsEntry::VerifiedNewDirectory { dir: new_dir, attr: new_attr })
                }
                AuthFsEntry::ReadonlyDirectory { .. } => {
                    Err(io::Error::from_raw_os_error(libc::EACCES))
                }
                _ => Err(io::Error::from_raw_os_error(libc::EBADF)),
            },
        )?;

        Ok(Entry {
            inode: new_inode,
            generation: 0,
            attr: create_dir_stat(new_inode, /* file_number */ 0, AccessMode::Variable(mode))?,
            entry_timeout: DEFAULT_METADATA_TIMEOUT,
            attr_timeout: DEFAULT_METADATA_TIMEOUT,
        })
    }

    fn unlink(&self, _ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<()> {
        let mut inode_table = self.inode_table.write().unwrap();
        handle_inode_mut_locked(
            &mut inode_table,
            &parent,
            |InodeState { entry, unlinked, .. }| match entry {
                AuthFsEntry::VerifiedNewDirectory { dir, .. } => {
                    let basename: &Path = cstr_to_path(name);
                    // Delete the file from in both the local and remote directories.
                    let _inode = dir.delete_file(basename)?;
                    *unlinked = true;
                    Ok(())
                }
                AuthFsEntry::ReadonlyDirectory { .. } => {
                    Err(io::Error::from_raw_os_error(libc::EACCES))
                }
                AuthFsEntry::VerifiedNew { .. } => {
                    // Deleting a entry in filesystem root is not currently supported.
                    Err(io::Error::from_raw_os_error(libc::ENOSYS))
                }
                AuthFsEntry::UnverifiedReadonly { .. } | AuthFsEntry::VerifiedReadonly { .. } => {
                    Err(io::Error::from_raw_os_error(libc::ENOTDIR))
                }
            },
        )
    }

    fn rmdir(&self, _ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<()> {
        let mut inode_table = self.inode_table.write().unwrap();

        // Check before actual removal, with readonly borrow.
        handle_inode_locked(&inode_table, &parent, |inode_state| match &inode_state.entry {
            AuthFsEntry::VerifiedNewDirectory { dir, .. } => {
                let basename: &Path = cstr_to_path(name);
                let existing_inode = dir.find_inode(basename)?;
                handle_inode_locked(&inode_table, &existing_inode, |inode_state| {
                    inode_state.entry.expect_empty_deletable_directory()
                })
            }
            AuthFsEntry::ReadonlyDirectory { .. } => {
                Err(io::Error::from_raw_os_error(libc::EACCES))
            }
            _ => Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        })?;

        // Look up again, this time with mutable borrow. This needs to be done separately because
        // the previous lookup needs to borrow multiple entry references in the table.
        handle_inode_mut_locked(
            &mut inode_table,
            &parent,
            |InodeState { entry, unlinked, .. }| match entry {
                AuthFsEntry::VerifiedNewDirectory { dir, .. } => {
                    let basename: &Path = cstr_to_path(name);
                    let _inode = dir.force_delete_directory(basename)?;
                    *unlinked = true;
                    Ok(())
                }
                _ => unreachable!("Mismatched entry type that is just checked"),
            },
        )
    }

    fn opendir(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _flags: u32,
    ) -> io::Result<(Option<Self::Handle>, FuseOpenOptions)> {
        let entries = self.handle_inode(&inode, |config| match config {
            AuthFsEntry::VerifiedNewDirectory { dir, .. } => dir.retrieve_entries(),
            AuthFsEntry::ReadonlyDirectory { dir } => dir.retrieve_entries(),
            _ => Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        })?;
        self.open_dir_store_snapshot(entries)
    }

    fn readdir(
        &self,
        _ctx: Context,
        _inode: Self::Inode,
        handle: Self::Handle,
        _size: u32,
        offset: u64,
    ) -> io::Result<Self::DirIter> {
        let dir_handle_table = self.dir_handle_table.read().unwrap();
        if let Some(entry) = dir_handle_table.get(&handle) {
            Ok(DirEntriesSnapshotIterator {
                snapshot: entry.clone(),
                prev_offset: offset.try_into().unwrap(),
            })
        } else {
            Err(io::Error::from_raw_os_error(libc::EBADF))
        }
    }

    fn releasedir(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _flags: u32,
        handle: Self::Handle,
    ) -> io::Result<()> {
        let mut dir_handle_table = self.dir_handle_table.write().unwrap();
        if dir_handle_table.remove(&handle).is_none() {
            unreachable!("Unknown directory handle {}, inode {}", handle, inode);
        }
        Ok(())
    }

    fn statfs(&self, _ctx: Context, _inode: Self::Inode) -> io::Result<libc::statvfs64> {
        let remote_stat = self.remote_fs_stats_reader.statfs()?;

        // SAFETY: We are zero-initializing a struct with only POD fields. Not all fields matter to
        // FUSE. See also:
        // https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/fs/fuse/inode.c?h=v5.15#n460
        let mut st: libc::statvfs64 = unsafe { zeroed() };

        // Use the remote stat as a template, since it'd matter the most to consider the writable
        // files/directories that are written to the remote.
        st.f_bsize = remote_stat.block_size;
        st.f_frsize = remote_stat.fragment_size;
        st.f_blocks = remote_stat.block_numbers;
        st.f_bavail = remote_stat.block_available;
        st.f_favail = remote_stat.inodes_available;
        st.f_namemax = remote_stat.max_filename;
        // Assuming we are not privileged to use all free spaces on the remote server, set the free
        // blocks/fragment to the same available amount.
        st.f_bfree = st.f_bavail;
        st.f_ffree = st.f_favail;
        // Number of inodes on the filesystem
        st.f_files = self.inode_table.read().unwrap().len() as u64;

        Ok(st)
    }
}

fn handle_inode_locked<F, R>(
    inode_table: &BTreeMap<Inode, InodeState>,
    inode: &Inode,
    handle_fn: F,
) -> io::Result<R>
where
    F: FnOnce(&InodeState) -> io::Result<R>,
{
    if let Some(inode_state) = inode_table.get(inode) {
        handle_fn(inode_state)
    } else {
        Err(io::Error::from_raw_os_error(libc::ENOENT))
    }
}

fn handle_inode_mut_locked<F, R>(
    inode_table: &mut BTreeMap<Inode, InodeState>,
    inode: &Inode,
    handle_fn: F,
) -> io::Result<R>
where
    F: FnOnce(&mut InodeState) -> io::Result<R>,
{
    if let Some(inode_state) = inode_table.get_mut(inode) {
        handle_fn(inode_state)
    } else {
        Err(io::Error::from_raw_os_error(libc::ENOENT))
    }
}

fn check_unsupported_setattr_request(valid: SetattrValid) -> io::Result<()> {
    if valid.contains(SetattrValid::UID) {
        warn!("Changing st_uid is not currently supported");
        return Err(io::Error::from_raw_os_error(libc::ENOSYS));
    }
    if valid.contains(SetattrValid::GID) {
        warn!("Changing st_gid is not currently supported");
        return Err(io::Error::from_raw_os_error(libc::ENOSYS));
    }
    if valid.intersects(
        SetattrValid::CTIME
            | SetattrValid::ATIME
            | SetattrValid::ATIME_NOW
            | SetattrValid::MTIME
            | SetattrValid::MTIME_NOW,
    ) {
        trace!("Ignoring ctime/atime/mtime change as authfs does not maintain timestamp currently");
    }
    Ok(())
}

fn cstr_to_path(cstr: &CStr) -> &Path {
    OsStr::from_bytes(cstr.to_bytes()).as_ref()
}
