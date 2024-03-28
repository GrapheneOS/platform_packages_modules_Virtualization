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

//! `zipfuse` is a FUSE filesystem for zip archives. It provides transparent access to the files
//! in a zip archive. This filesystem does not supporting writing files back to the zip archive.
//! The filesystem has to be mounted read only.

mod inode;

use anyhow::{Context as AnyhowContext, Result};
use clap::{builder::ValueParser, Arg, ArgAction, Command};
use fuse::filesystem::*;
use fuse::mount::*;
use rustutils::system_properties;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io;
use std::io::Read;
use std::mem::{size_of, MaybeUninit};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::inode::{DirectoryEntry, Inode, InodeData, InodeKind, InodeTable};

fn main() -> Result<()> {
    let matches = clap_command().get_matches();

    let zip_file = matches.get_one::<PathBuf>("ZIPFILE").unwrap();
    let mount_point = matches.get_one::<PathBuf>("MOUNTPOINT").unwrap();
    let options = matches.get_one::<String>("options");
    let noexec = matches.get_flag("noexec");
    let ready_prop = matches.get_one::<String>("readyprop");
    let uid: u32 = matches.get_one::<String>("uid").map_or(0, |s| s.parse().unwrap());
    let gid: u32 = matches.get_one::<String>("gid").map_or(0, |s| s.parse().unwrap());
    run_fuse(zip_file, mount_point, options, noexec, ready_prop, uid, gid)?;

    Ok(())
}

fn clap_command() -> Command {
    Command::new("zipfuse")
        .arg(
            Arg::new("options")
                .short('o')
                .required(false)
                .help("Comma separated list of mount options"),
        )
        .arg(
            Arg::new("noexec")
                .long("noexec")
                .action(ArgAction::SetTrue)
                .help("Disallow the execution of binary files"),
        )
        .arg(
            Arg::new("readyprop")
                .short('p')
                .help("Specify a property to be set when mount is ready"),
        )
        .arg(Arg::new("uid").short('u').help("numeric UID who's the owner of the files"))
        .arg(Arg::new("gid").short('g').help("numeric GID who's the group of the files"))
        .arg(Arg::new("ZIPFILE").value_parser(ValueParser::path_buf()).required(true))
        .arg(Arg::new("MOUNTPOINT").value_parser(ValueParser::path_buf()).required(true))
}

/// Runs a fuse filesystem by mounting `zip_file` on `mount_point`.
pub fn run_fuse(
    zip_file: &Path,
    mount_point: &Path,
    extra_options: Option<&String>,
    noexec: bool,
    ready_prop: Option<&String>,
    uid: u32,
    gid: u32,
) -> Result<()> {
    const MAX_READ: u32 = 1 << 20; // TODO(jiyong): tune this
    const MAX_WRITE: u32 = 1 << 13; // This is a read-only filesystem

    let dev_fuse = OpenOptions::new().read(true).write(true).open("/dev/fuse")?;

    let mut mount_options = vec![
        MountOption::FD(dev_fuse.as_raw_fd()),
        MountOption::DefaultPermissions,
        MountOption::RootMode(libc::S_IFDIR | libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH),
        MountOption::AllowOther,
        MountOption::UserId(0),
        MountOption::GroupId(0),
        MountOption::MaxRead(MAX_READ),
    ];
    if let Some(value) = extra_options {
        mount_options.push(MountOption::Extra(value));
    }

    let mut mount_flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_RDONLY;
    if noexec {
        mount_flags |= libc::MS_NOEXEC;
    }

    fuse::mount(mount_point, "zipfuse", mount_flags, &mount_options)?;

    if let Some(property_name) = ready_prop {
        system_properties::write(property_name, "1").context("Failed to set readyprop")?;
    }

    let mut config = fuse::FuseConfig::new();
    config.dev_fuse(dev_fuse).max_write(MAX_WRITE).max_read(MAX_READ);
    Ok(config.enter_message_loop(ZipFuse::new(zip_file, uid, gid)?)?)
}

struct ZipFuse {
    zip_archive: Mutex<zip::ZipArchive<File>>,
    raw_file: Mutex<File>,
    inode_table: InodeTable,
    open_files: Mutex<HashMap<Handle, OpenFile>>,
    open_dirs: Mutex<HashMap<Handle, OpenDirBuf>>,
    uid: u32,
    gid: u32,
}

/// Represents a [`ZipFile`] that is opened.
struct OpenFile {
    open_count: u32, // multiple opens share the buf because this is a read-only filesystem
    content: OpenFileContent,
}

/// Holds the content of a [`ZipFile`]. Depending on whether it is compressed or not, the
/// entire content is stored, or only the zip index is stored.
enum OpenFileContent {
    Compressed(Box<[u8]>),
    Uncompressed(usize), // zip index
}

/// Holds the directory entries in a directory opened by [`opendir`].
struct OpenDirBuf {
    open_count: u32,
    buf: Box<[(CString, DirectoryEntry)]>,
}

type Handle = u64;

fn ebadf() -> io::Error {
    io::Error::from_raw_os_error(libc::EBADF)
}

fn timeout_max() -> std::time::Duration {
    std::time::Duration::new(u64::MAX, 1_000_000_000 - 1)
}

impl ZipFuse {
    fn new(zip_file: &Path, uid: u32, gid: u32) -> Result<ZipFuse> {
        // TODO(jiyong): Use O_DIRECT to avoid double caching.
        // `.custom_flags(nix::fcntl::OFlag::O_DIRECT.bits())` currently doesn't work.
        let f = File::open(zip_file)?;
        let mut z = zip::ZipArchive::new(f)?;
        // Open the same file again so that we can directly access it when accessing
        // uncompressed zip_file entries in it. `ZipFile` doesn't implement `Seek`.
        let raw_file = File::open(zip_file)?;
        let it = InodeTable::from_zip(&mut z)?;
        Ok(ZipFuse {
            zip_archive: Mutex::new(z),
            raw_file: Mutex::new(raw_file),
            inode_table: it,
            open_files: Mutex::new(HashMap::new()),
            open_dirs: Mutex::new(HashMap::new()),
            uid,
            gid,
        })
    }

    fn find_inode(&self, inode: Inode) -> io::Result<&InodeData> {
        self.inode_table.get(inode).ok_or_else(ebadf)
    }

    // TODO(jiyong) remove this. Right now this is needed to do the nlink_t to u64 conversion below
    // on aosp_x86_64 target. That however is a useless conversion on other targets.
    #[allow(clippy::useless_conversion)]
    fn stat_from(&self, inode: Inode) -> io::Result<libc::stat64> {
        let inode_data = self.find_inode(inode)?;
        // SAFETY: All fields of stat64 are valid for zero byte patterns.
        let mut st = unsafe { MaybeUninit::<libc::stat64>::zeroed().assume_init() };
        st.st_dev = 0;
        st.st_nlink = if let Some(directory) = inode_data.get_directory() {
            (2 + directory.len() as libc::nlink_t).into()
        } else {
            1
        };
        st.st_ino = inode;
        st.st_mode = if inode_data.is_dir() { libc::S_IFDIR } else { libc::S_IFREG };
        st.st_mode |= inode_data.mode;
        st.st_uid = self.uid;
        st.st_gid = self.gid;
        st.st_size = i64::try_from(inode_data.size).unwrap_or(i64::MAX);
        Ok(st)
    }
}

impl fuse::filesystem::FileSystem for ZipFuse {
    type Inode = Inode;
    type Handle = Handle;
    type DirIter = DirIter;

    fn init(&self, _capable: FsOptions) -> std::io::Result<FsOptions> {
        // The default options added by the fuse crate are fine. We don't have additional options.
        Ok(FsOptions::empty())
    }

    fn lookup(&self, _ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<Entry> {
        let inode = self.find_inode(parent)?;
        let directory = inode.get_directory().ok_or_else(ebadf)?;
        let entry = directory.get(name);
        match entry {
            Some(e) => Ok(Entry {
                inode: e.inode,
                generation: 0,
                attr: self.stat_from(e.inode)?,
                attr_timeout: timeout_max(), // this is a read-only fs
                entry_timeout: timeout_max(),
            }),
            _ => Err(io::Error::from_raw_os_error(libc::ENOENT)),
        }
    }

    fn getattr(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _handle: Option<Self::Handle>,
    ) -> io::Result<(libc::stat64, std::time::Duration)> {
        let st = self.stat_from(inode)?;
        Ok((st, timeout_max()))
    }

    fn open(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _flags: u32,
    ) -> io::Result<(Option<Self::Handle>, fuse::filesystem::OpenOptions)> {
        let mut open_files = self.open_files.lock().unwrap();
        let handle = inode as Handle;

        // If the file is already opened, just increase the reference counter. If not, read the
        // entire file content to the buffer. When `read` is called, a portion of the buffer is
        // copied to the kernel.
        if let Some(file) = open_files.get_mut(&handle) {
            if file.open_count == 0 {
                return Err(ebadf());
            }
            file.open_count += 1;
        } else {
            let inode_data = self.find_inode(inode)?;
            let zip_index = inode_data.get_zip_index().ok_or_else(ebadf)?;
            let mut zip_archive = self.zip_archive.lock().unwrap();
            let mut zip_file = zip_archive.by_index(zip_index)?;
            let content = match zip_file.compression() {
                zip::CompressionMethod::Stored => OpenFileContent::Uncompressed(zip_index),
                _ => {
                    if let Some(mode) = zip_file.unix_mode() {
                        let is_reg_file = zip_file.is_file();
                        let is_executable =
                            mode & (libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH) != 0;
                        if is_reg_file && is_executable {
                            log::warn!(
                                "Executable file {:?} is stored compressed. Consider \
                                storing it uncompressed to save memory",
                                zip_file.mangled_name()
                            );
                        }
                    }
                    let mut buf = Vec::with_capacity(inode_data.size as usize);
                    zip_file.read_to_end(&mut buf)?;
                    OpenFileContent::Compressed(buf.into_boxed_slice())
                }
            };
            open_files.insert(handle, OpenFile { open_count: 1, content });
        }
        // Note: we don't return `DIRECT_IO` here, because then applications wouldn't be able to
        // mmap the files.
        Ok((Some(handle), fuse::filesystem::OpenOptions::empty()))
    }

    fn release(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _flags: u32,
        _handle: Self::Handle,
        _flush: bool,
        _flock_release: bool,
        _lock_owner: Option<u64>,
    ) -> io::Result<()> {
        // Releases the buffer for the `handle` when it is opened for nobody. While this is good
        // for saving memory, this has a performance implication because we need to decompress
        // again when the same file is opened in the future.
        let mut open_files = self.open_files.lock().unwrap();
        let handle = inode as Handle;
        if let Some(file) = open_files.get_mut(&handle) {
            if file.open_count.checked_sub(1).ok_or_else(ebadf)? == 0 {
                open_files.remove(&handle);
            }
            Ok(())
        } else {
            Err(ebadf())
        }
    }

    #[allow(clippy::unused_io_amount)]
    fn read<W: io::Write + ZeroCopyWriter>(
        &self,
        _ctx: Context,
        _inode: Self::Inode,
        handle: Self::Handle,
        mut w: W,
        size: u32,
        offset: u64,
        _lock_owner: Option<u64>,
        _flags: u32,
    ) -> io::Result<usize> {
        let open_files = self.open_files.lock().unwrap();
        let file = open_files.get(&handle).ok_or_else(ebadf)?;
        if file.open_count == 0 {
            return Err(ebadf());
        }
        Ok(match &file.content {
            OpenFileContent::Uncompressed(zip_index) => {
                let mut zip_archive = self.zip_archive.lock().unwrap();
                let zip_file = zip_archive.by_index(*zip_index)?;
                let start = zip_file.data_start() + offset;
                let remaining_size = zip_file.size() - offset;
                let size = std::cmp::min(remaining_size, size.into());

                let mut raw_file = self.raw_file.lock().unwrap();
                w.write_from(&mut raw_file, size as usize, start)?
            }
            OpenFileContent::Compressed(buf) => {
                let start = offset as usize;
                let end = start + size as usize;
                let end = std::cmp::min(end, buf.len());
                w.write(&buf[start..end])?
            }
        })
    }

    fn opendir(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _flags: u32,
    ) -> io::Result<(Option<Self::Handle>, fuse::filesystem::OpenOptions)> {
        let mut open_dirs = self.open_dirs.lock().unwrap();
        let handle = inode as Handle;
        if let Some(odb) = open_dirs.get_mut(&handle) {
            if odb.open_count == 0 {
                return Err(ebadf());
            }
            odb.open_count += 1;
        } else {
            let inode_data = self.find_inode(inode)?;
            let directory = inode_data.get_directory().ok_or_else(ebadf)?;
            let mut buf: Vec<(CString, DirectoryEntry)> = Vec::with_capacity(directory.len());
            for (name, dir_entry) in directory.iter() {
                let name = CString::new(name.as_bytes()).unwrap();
                buf.push((name, dir_entry.clone()));
            }
            open_dirs.insert(handle, OpenDirBuf { open_count: 1, buf: buf.into_boxed_slice() });
        }
        Ok((Some(handle), fuse::filesystem::OpenOptions::CACHE_DIR))
    }

    fn releasedir(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _flags: u32,
        _handle: Self::Handle,
    ) -> io::Result<()> {
        let mut open_dirs = self.open_dirs.lock().unwrap();
        let handle = inode as Handle;
        if let Some(odb) = open_dirs.get_mut(&handle) {
            if odb.open_count.checked_sub(1).ok_or_else(ebadf)? == 0 {
                open_dirs.remove(&handle);
            }
            Ok(())
        } else {
            Err(ebadf())
        }
    }

    fn readdir(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _handle: Self::Handle,
        size: u32,
        offset: u64,
    ) -> io::Result<Self::DirIter> {
        let open_dirs = self.open_dirs.lock().unwrap();
        let handle = inode as Handle;
        let odb = open_dirs.get(&handle).ok_or_else(ebadf)?;
        if odb.open_count == 0 {
            return Err(ebadf());
        }
        let buf = &odb.buf;
        let start = offset as usize;

        // Estimate the size of each entry will take space in the buffer. See
        // external/crosvm/fuse/src/server.rs#add_dirent
        let mut estimate: usize = 0; // estimated number of bytes we will be writing
        let mut end = start; // index in `buf`
        while estimate < size as usize && end < buf.len() {
            let dirent_size = size_of::<fuse::sys::Dirent>();
            let name_size = buf[end].0.to_bytes().len();
            estimate += (dirent_size + name_size + 7) & !7; // round to 8 byte boundary
            end += 1;
        }

        let mut new_buf = Vec::with_capacity(end - start);
        // The portion of `buf` is *copied* to the iterator. This is not ideal, but inevitable
        // because the `name` field in `fuse::filesystem::DirEntry` is `&CStr` not `CString`.
        new_buf.extend_from_slice(&buf[start..end]);
        Ok(DirIter { inner: new_buf, offset, cur: 0 })
    }
}

struct DirIter {
    inner: Vec<(CString, DirectoryEntry)>,
    offset: u64, // the offset where this iterator begins. `next` doesn't change this.
    cur: usize,  // the current index in `inner`. `next` advances this.
}

impl fuse::filesystem::DirectoryIterator for DirIter {
    fn next(&mut self) -> Option<fuse::filesystem::DirEntry> {
        if self.cur >= self.inner.len() {
            return None;
        }

        let (name, entry) = &self.inner[self.cur];
        self.cur += 1;
        Some(fuse::filesystem::DirEntry {
            ino: entry.inode as libc::ino64_t,
            offset: self.offset + self.cur as u64,
            type_: match entry.kind {
                InodeKind::Directory => libc::DT_DIR.into(),
                InodeKind::File => libc::DT_REG.into(),
            },
            name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use nix::sys::statfs::{statfs, FsType};
    use std::collections::BTreeSet;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};
    use zip::write::FileOptions;

    #[derive(Default)]
    struct Options {
        noexec: bool,
        uid: u32,
        gid: u32,
    }

    #[cfg(not(target_os = "android"))]
    fn start_fuse(zip_path: &Path, mnt_path: &Path, opt: Options) {
        let zip_path = PathBuf::from(zip_path);
        let mnt_path = PathBuf::from(mnt_path);
        std::thread::spawn(move || {
            crate::run_fuse(&zip_path, &mnt_path, None, opt.noexec, opt.uid, opt.gid).unwrap();
        });
    }

    #[cfg(target_os = "android")]
    fn start_fuse(zip_path: &Path, mnt_path: &Path, opt: Options) {
        // Note: for some unknown reason, running a thread to serve fuse doesn't work on Android.
        // Explicitly spawn a zipfuse process instead.
        // TODO(jiyong): fix this
        let noexec = if opt.noexec { "--noexec" } else { "" };
        assert!(std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "/data/local/tmp/zipfuse {} -u {} -g {} {} {}",
                noexec,
                opt.uid,
                opt.gid,
                zip_path.display(),
                mnt_path.display()
            ))
            .spawn()
            .is_ok());
    }

    fn wait_for_mount(mount_path: &Path) -> Result<()> {
        let start_time = Instant::now();
        const POLL_INTERVAL: Duration = Duration::from_millis(50);
        const TIMEOUT: Duration = Duration::from_secs(10);
        const FUSE_SUPER_MAGIC: FsType = FsType(0x65735546);
        loop {
            if statfs(mount_path)?.filesystem_type() == FUSE_SUPER_MAGIC {
                break;
            }

            if start_time.elapsed() > TIMEOUT {
                bail!("Time out mounting zipfuse");
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        Ok(())
    }

    // Creates a zip file, adds some files to the zip file, mounts it using zipfuse, runs the check
    // routine, and finally unmounts.
    fn run_test(add: fn(&mut zip::ZipWriter<File>), check: fn(&std::path::Path)) {
        run_test_with_options(Default::default(), add, check);
    }

    fn run_test_with_options(
        opt: Options,
        add: fn(&mut zip::ZipWriter<File>),
        check: fn(&std::path::Path),
    ) {
        // Create an empty zip file
        let test_dir = tempfile::TempDir::new().unwrap();
        let zip_path = test_dir.path().join("test.zip");
        let zip = File::create(&zip_path);
        assert!(zip.is_ok());
        let mut zip = zip::ZipWriter::new(zip.unwrap());

        // Let test users add files/dirs to the zip file
        add(&mut zip);
        assert!(zip.finish().is_ok());
        drop(zip);

        // Mount the zip file on the "mnt" dir using zipfuse.
        let mnt_path = test_dir.path().join("mnt");
        assert!(fs::create_dir(&mnt_path).is_ok());

        start_fuse(&zip_path, &mnt_path, opt);

        let mnt_path = test_dir.path().join("mnt");
        // Give some time for the fuse to boot up
        assert!(wait_for_mount(&mnt_path).is_ok());
        // Run the check routine, and do the clean up.
        check(&mnt_path);
        assert!(nix::mount::umount2(&mnt_path, nix::mount::MntFlags::empty()).is_ok());
    }

    fn check_file(root: &Path, file: &str, content: &[u8]) {
        let path = root.join(file);
        assert!(path.exists());

        let metadata = fs::metadata(&path);
        assert!(metadata.is_ok());

        let metadata = metadata.unwrap();
        assert!(metadata.is_file());
        assert_eq!(content.len(), metadata.len() as usize);

        let read_data = fs::read(&path);
        assert!(read_data.is_ok());
        assert_eq!(content, read_data.unwrap().as_slice());
    }

    fn check_dir<S: AsRef<str>>(root: &Path, dir: &str, files: &[S], dirs: &[S]) {
        let dir_path = root.join(dir);
        assert!(dir_path.exists());

        let metadata = fs::metadata(&dir_path);
        assert!(metadata.is_ok());

        let metadata = metadata.unwrap();
        assert!(metadata.is_dir());

        let iter = fs::read_dir(&dir_path);
        assert!(iter.is_ok());

        let iter = iter.unwrap();
        let mut actual_files = BTreeSet::new();
        let mut actual_dirs = BTreeSet::new();
        for de in iter {
            let entry = de.unwrap();
            let path = entry.path();
            if path.is_dir() {
                actual_dirs.insert(path.strip_prefix(&dir_path).unwrap().to_path_buf());
            } else {
                actual_files.insert(path.strip_prefix(&dir_path).unwrap().to_path_buf());
            }
        }
        let expected_files: BTreeSet<PathBuf> =
            files.iter().map(|s| PathBuf::from(s.as_ref())).collect();
        let expected_dirs: BTreeSet<PathBuf> =
            dirs.iter().map(|s| PathBuf::from(s.as_ref())).collect();

        assert_eq!(expected_files, actual_files);
        assert_eq!(expected_dirs, actual_dirs);
    }

    #[test]
    fn empty() {
        run_test(
            |_| {},
            |root| {
                check_dir::<String>(root, "", &[], &[]);
            },
        );
    }

    #[test]
    fn single_file() {
        run_test(
            |zip| {
                zip.start_file("foo", FileOptions::default()).unwrap();
                zip.write_all(b"0123456789").unwrap();
            },
            |root| {
                check_dir(root, "", &["foo"], &[]);
                check_file(root, "foo", b"0123456789");
            },
        );
    }

    #[test]
    fn noexec() {
        fn add_executable(zip: &mut zip::ZipWriter<File>) {
            zip.start_file("executable", FileOptions::default().unix_permissions(0o755)).unwrap();
        }

        // Executables can be run when not mounting with noexec.
        run_test(add_executable, |root| {
            let res = std::process::Command::new(root.join("executable")).status();
            res.unwrap();
        });

        // Mounting with noexec results in permissions denial when running an executable.
        let opt = Options { noexec: true, ..Default::default() };
        run_test_with_options(opt, add_executable, |root| {
            let res = std::process::Command::new(root.join("executable")).status();
            assert!(matches!(res.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied));
        });
    }

    #[test]
    fn uid_gid() {
        const UID: u32 = 100;
        const GID: u32 = 200;
        run_test_with_options(
            Options { noexec: true, uid: UID, gid: GID },
            |zip| {
                zip.start_file("foo", FileOptions::default()).unwrap();
                zip.write_all(b"0123456789").unwrap();
            },
            |root| {
                let path = root.join("foo");

                let metadata = fs::metadata(path);
                assert!(metadata.is_ok());
                let metadata = metadata.unwrap();

                assert_eq!(UID, metadata.uid());
                assert_eq!(GID, metadata.gid());
            },
        );
    }

    #[test]
    fn single_dir() {
        run_test(
            |zip| {
                zip.add_directory("dir", FileOptions::default()).unwrap();
            },
            |root| {
                check_dir(root, "", &[], &["dir"]);
                check_dir::<String>(root, "dir", &[], &[]);
            },
        );
    }

    #[test]
    fn complex_hierarchy() {
        // root/
        //   a/
        //    b1/
        //    b2/
        //      c1 (file)
        //      c2/
        //          d1 (file)
        //          d2 (file)
        //          d3 (file)
        //  x/
        //    y1 (file)
        //    y2 (file)
        //    y3/
        //
        //  foo (file)
        //  bar (file)
        run_test(
            |zip| {
                let opt = FileOptions::default();
                zip.add_directory("a/b1", opt).unwrap();

                zip.start_file("a/b2/c1", opt).unwrap();

                zip.start_file("a/b2/c2/d1", opt).unwrap();
                zip.start_file("a/b2/c2/d2", opt).unwrap();
                zip.start_file("a/b2/c2/d3", opt).unwrap();

                zip.start_file("x/y1", opt).unwrap();
                zip.start_file("x/y2", opt).unwrap();
                zip.add_directory("x/y3", opt).unwrap();

                zip.start_file("foo", opt).unwrap();
                zip.start_file("bar", opt).unwrap();
            },
            |root| {
                check_dir(root, "", &["foo", "bar"], &["a", "x"]);
                check_dir(root, "a", &[], &["b1", "b2"]);
                check_dir::<String>(root, "a/b1", &[], &[]);
                check_dir(root, "a/b2", &["c1"], &["c2"]);
                check_dir(root, "a/b2/c2", &["d1", "d2", "d3"], &[]);
                check_dir(root, "x", &["y1", "y2"], &["y3"]);
                check_dir::<String>(root, "x/y3", &[], &[]);
                check_file(root, "a/b2/c1", &[]);
                check_file(root, "a/b2/c2/d1", &[]);
                check_file(root, "a/b2/c2/d2", &[]);
                check_file(root, "a/b2/c2/d3", &[]);
                check_file(root, "x/y1", &[]);
                check_file(root, "x/y2", &[]);
                check_file(root, "foo", &[]);
                check_file(root, "bar", &[]);
            },
        );
    }

    #[test]
    fn large_file() {
        run_test(
            |zip| {
                let data = vec![10; 2 << 20];
                zip.start_file("foo", FileOptions::default()).unwrap();
                zip.write_all(&data).unwrap();
            },
            |root| {
                let data = vec![10; 2 << 20];
                check_file(root, "foo", &data);
            },
        );
    }

    #[test]
    fn large_dir() {
        const NUM_FILES: usize = 1 << 10;
        run_test(
            |zip| {
                let opt = FileOptions::default();
                // create 1K files. Each file has a name of length 100. So total size is at least
                // 100KB, which is bigger than the readdir buffer size of 4K.
                for i in 0..NUM_FILES {
                    zip.start_file(format!("dir/{:0100}", i), opt).unwrap();
                }
            },
            |root| {
                let dirs_expected: Vec<_> = (0..NUM_FILES).map(|i| format!("{:0100}", i)).collect();
                check_dir(
                    root,
                    "dir",
                    dirs_expected.iter().map(|s| s.as_str()).collect::<Vec<&str>>().as_slice(),
                    &[],
                );
            },
        );
    }

    fn run_fuse_and_check_test_zip(test_dir: &Path, zip_path: &Path) {
        let mnt_path = test_dir.join("mnt");
        assert!(fs::create_dir(&mnt_path).is_ok());

        let opt = Options { noexec: false, ..Default::default() };
        start_fuse(zip_path, &mnt_path, opt);

        // Give some time for the fuse to boot up
        assert!(wait_for_mount(&mnt_path).is_ok());

        check_dir(&mnt_path, "", &[], &["dir"]);
        check_dir(&mnt_path, "dir", &["file1", "file2"], &[]);
        check_file(&mnt_path, "dir/file1", include_bytes!("../testdata/dir/file1"));
        check_file(&mnt_path, "dir/file2", include_bytes!("../testdata/dir/file2"));
        assert!(nix::mount::umount2(&mnt_path, nix::mount::MntFlags::empty()).is_ok());
    }

    #[test]
    fn supports_deflate() {
        let test_dir = tempfile::TempDir::new().unwrap();
        let zip_path = test_dir.path().join("test.zip");
        let mut zip_file = File::create(&zip_path).unwrap();
        zip_file.write_all(include_bytes!("../testdata/test.zip")).unwrap();

        run_fuse_and_check_test_zip(test_dir.path(), &zip_path);
    }

    #[test]
    fn supports_store() {
        run_test(
            |zip| {
                let data = vec![10; 2 << 20];
                zip.start_file(
                    "foo",
                    FileOptions::default().compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
                zip.write_all(&data).unwrap();
            },
            |root| {
                let data = vec![10; 2 << 20];
                check_file(root, "foo", &data);
            },
        );
    }

    #[cfg(not(target_os = "android"))] // Android doesn't have the loopdev crate
    #[test]
    fn supports_zip_on_block_device() {
        // Write test.zip to the test directory
        let test_dir = tempfile::TempDir::new().unwrap();
        let zip_path = test_dir.path().join("test.zip");
        let mut zip_file = File::create(&zip_path).unwrap();
        let data = include_bytes!("../testdata/test.zip");
        zip_file.write_all(data).unwrap();

        // Pad 0 to test.zip so that its size is multiple of 4096.
        const BLOCK_SIZE: usize = 4096;
        let size = (data.len() + BLOCK_SIZE) & !BLOCK_SIZE;
        let pad_size = size - data.len();
        assert!(pad_size != 0);
        let pad = vec![0; pad_size];
        zip_file.write_all(pad.as_slice()).unwrap();
        drop(zip_file);

        // Attach test.zip to a loop device
        let lc = loopdev::LoopControl::open().unwrap();
        let ld = scopeguard::guard(lc.next_free().unwrap(), |ld| {
            ld.detach().unwrap();
        });
        ld.attach_file(&zip_path).unwrap();

        // Start zipfuse over to the loop device (not the zip file)
        run_fuse_and_check_test_zip(&test_dir.path(), &ld.path().unwrap());
    }

    #[test]
    fn verify_command() {
        // Check that the command parsing has been configured in a valid way.
        clap_command().debug_assert();
    }
}
