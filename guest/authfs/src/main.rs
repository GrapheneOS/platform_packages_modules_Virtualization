/*
 * Copyright (C) 2020 The Android Open Source Project
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

//! This crate implements AuthFS, a FUSE-based, non-generic filesystem where file access is
//! authenticated. This filesystem assumes the underlying layer is not trusted, e.g. file may be
//! provided by an untrusted host/VM, so that the content can't be simply trusted. However, with a
//! known file hash from trusted party, this filesystem can still verify a (read-only) file even if
//! the host/VM as the blob provider is malicious. With the Merkle tree, each read of file block can
//! be verified individually only when needed.
//!
//! AuthFS only serve files that are specifically configured. Each remote file can be configured to
//! appear as a local file at the mount point. A file configuration may include its remote file
//! identifier and its verification method (e.g. by known digest).
//!
//! AuthFS also support remote directories. A remote directory may be defined by a manifest file,
//! which contains file paths and their corresponding digests.
//!
//! AuthFS can also be configured for write, in which case the remote file server is treated as a
//! (untrusted) storage. The file/directory integrity is maintained in memory in the VM. Currently,
//! the state is not persistent, thus only new file/directory are supported.

use anyhow::{anyhow, bail, Result};
use clap::Parser;
use log::error;
use protobuf::Message;
use std::convert::TryInto;
use std::fs::File;
use std::num::NonZeroU8;
use std::path::{Path, PathBuf};

mod common;
mod file;
mod fsstat;
mod fsverity;
mod fusefs;

use file::{Attr, InMemoryDir, RemoteDirEditor, RemoteFileEditor, RemoteFileReader};
use fsstat::RemoteFsStatsReader;
use fsverity::VerifiedFileEditor;
use fsverity_digests_proto::fsverity_digests::FSVerityDigests;
use fusefs::{AuthFs, AuthFsEntry, LazyVerifiedReadonlyFile};

#[derive(Parser)]
struct Args {
    /// Mount point of AuthFS.
    mount_point: PathBuf,

    /// CID of the VM where the service runs.
    #[clap(long)]
    cid: u32,

    /// Extra options to FUSE
    #[clap(short = 'o')]
    extra_options: Option<String>,

    /// Number of threads to serve FUSE requests.
    #[clap(short = 'j')]
    thread_number: Option<NonZeroU8>,

    /// A read-only remote file with integrity check. Can be multiple.
    ///
    /// For example, `--remote-ro-file 5:sha256-1234abcd` tells the filesystem to associate the
    /// file $MOUNTPOINT/5 with a remote FD 5, and has a fs-verity digest with sha256 of the hex
    /// value 1234abcd.
    #[clap(long, value_parser = parse_remote_ro_file_option)]
    remote_ro_file: Vec<OptionRemoteRoFile>,

    /// A read-only remote file without integrity check. Can be multiple.
    ///
    /// For example, `--remote-ro-file-unverified 5` tells the filesystem to associate the file
    /// $MOUNTPOINT/5 with a remote FD 5.
    #[clap(long)]
    remote_ro_file_unverified: Vec<i32>,

    /// A new read-writable remote file with integrity check. Can be multiple.
    ///
    /// For example, `--remote-new-rw-file 5` tells the filesystem to associate the file
    /// $MOUNTPOINT/5 with a remote FD 5.
    #[clap(long)]
    remote_new_rw_file: Vec<i32>,

    /// A read-only directory that represents a remote directory. The directory view is constructed
    /// and finalized during the filesystem initialization based on the provided mapping file
    /// (which is a serialized protobuf of android.security.fsverity.FSVerityDigests, which
    /// essentially provides <file path, fs-verity digest> mappings of exported files). The mapping
    /// file is supposed to come from a trusted location in order to provide a trusted view as well
    /// as verified access of included files with their fs-verity digest. Not all files on the
    /// remote host may be included in the mapping file, so the directory view may be partial. The
    /// directory structure won't change throughout the filesystem lifetime.
    ///
    /// For example, `--remote-ro-dir 5:/path/to/mapping:prefix/` tells the filesystem to
    /// construct a directory structure defined in the mapping file at $MOUNTPOINT/5, which may
    /// include a file like /5/system/framework/framework.jar. "prefix/" tells the filesystem to
    /// strip the path (e.g. "system/") from the mount point to match the expected location of the
    /// remote FD (e.g. a directory FD of "/system" in the remote).
    #[clap(long, value_parser = parse_remote_new_ro_dir_option)]
    remote_ro_dir: Vec<OptionRemoteRoDir>,

    /// A new directory that is assumed empty in the backing filesystem. New files created in this
    /// directory are integrity-protected in the same way as --remote-new-verified-file. Can be
    /// multiple.
    ///
    /// For example, `--remote-new-rw-dir 5` tells the filesystem to associate $MOUNTPOINT/5
    /// with a remote dir FD 5.
    #[clap(long)]
    remote_new_rw_dir: Vec<i32>,

    /// Enable debugging features.
    #[clap(long)]
    debug: bool,
}

#[derive(Clone)]
struct OptionRemoteRoFile {
    /// ID to refer to the remote file.
    remote_fd: i32,

    /// Expected fs-verity digest (with sha256) for the remote file.
    digest: String,
}

#[derive(Clone)]
struct OptionRemoteRoDir {
    /// ID to refer to the remote dir.
    remote_dir_fd: i32,

    /// A mapping file that describes the expecting file/directory structure and integrity metadata
    /// in the remote directory. The file contains serialized protobuf of
    /// android.security.fsverity.FSVerityDigests.
    mapping_file_path: PathBuf,

    prefix: String,
}

fn parse_remote_ro_file_option(option: &str) -> Result<OptionRemoteRoFile> {
    let strs: Vec<&str> = option.split(':').collect();
    if strs.len() != 2 {
        bail!("Invalid option: {}", option);
    }
    if let Some(digest) = strs[1].strip_prefix("sha256-") {
        Ok(OptionRemoteRoFile { remote_fd: strs[0].parse::<i32>()?, digest: String::from(digest) })
    } else {
        bail!("Unsupported hash algorithm or invalid format: {}", strs[1]);
    }
}

fn parse_remote_new_ro_dir_option(option: &str) -> Result<OptionRemoteRoDir> {
    let strs: Vec<&str> = option.split(':').collect();
    if strs.len() != 3 {
        bail!("Invalid option: {}", option);
    }
    Ok(OptionRemoteRoDir {
        remote_dir_fd: strs[0].parse::<i32>().unwrap(),
        mapping_file_path: PathBuf::from(strs[1]),
        prefix: String::from(strs[2]),
    })
}

fn new_remote_verified_file_entry(
    service: file::VirtFdService,
    remote_fd: i32,
    expected_digest: &str,
) -> Result<AuthFsEntry> {
    Ok(AuthFsEntry::VerifiedReadonly {
        reader: LazyVerifiedReadonlyFile::prepare_by_fd(
            service,
            remote_fd,
            hex::decode(expected_digest)?,
        ),
    })
}

fn new_remote_unverified_file_entry(
    service: file::VirtFdService,
    remote_fd: i32,
    file_size: u64,
) -> Result<AuthFsEntry> {
    let reader = RemoteFileReader::new(service, remote_fd);
    Ok(AuthFsEntry::UnverifiedReadonly { reader, file_size })
}

fn new_remote_new_verified_file_entry(
    service: file::VirtFdService,
    remote_fd: i32,
) -> Result<AuthFsEntry> {
    let remote_file = RemoteFileEditor::new(service.clone(), remote_fd);
    Ok(AuthFsEntry::VerifiedNew {
        editor: VerifiedFileEditor::new(remote_file),
        attr: Attr::new_file(service, remote_fd),
    })
}

fn new_remote_new_verified_dir_entry(
    service: file::VirtFdService,
    remote_fd: i32,
) -> Result<AuthFsEntry> {
    let dir = RemoteDirEditor::new(service.clone(), remote_fd);
    let attr = Attr::new_dir(service, remote_fd);
    Ok(AuthFsEntry::VerifiedNewDirectory { dir, attr })
}

fn prepare_root_dir_entries(
    service: file::VirtFdService,
    authfs: &mut AuthFs,
    args: &Args,
) -> Result<()> {
    for config in &args.remote_ro_file {
        authfs.add_entry_at_root_dir(
            remote_fd_to_path_buf(config.remote_fd),
            new_remote_verified_file_entry(service.clone(), config.remote_fd, &config.digest)?,
        )?;
    }

    for remote_fd in &args.remote_ro_file_unverified {
        let remote_fd = *remote_fd;
        authfs.add_entry_at_root_dir(
            remote_fd_to_path_buf(remote_fd),
            new_remote_unverified_file_entry(
                service.clone(),
                remote_fd,
                service.getFileSize(remote_fd)?.try_into()?,
            )?,
        )?;
    }

    for remote_fd in &args.remote_new_rw_file {
        let remote_fd = *remote_fd;
        authfs.add_entry_at_root_dir(
            remote_fd_to_path_buf(remote_fd),
            new_remote_new_verified_file_entry(service.clone(), remote_fd)?,
        )?;
    }

    for remote_fd in &args.remote_new_rw_dir {
        let remote_fd = *remote_fd;
        authfs.add_entry_at_root_dir(
            remote_fd_to_path_buf(remote_fd),
            new_remote_new_verified_dir_entry(service.clone(), remote_fd)?,
        )?;
    }

    for config in &args.remote_ro_dir {
        let dir_root_inode = authfs.add_entry_at_root_dir(
            remote_fd_to_path_buf(config.remote_dir_fd),
            AuthFsEntry::ReadonlyDirectory { dir: InMemoryDir::new() },
        )?;

        // Build the directory tree based on the mapping file.
        let mut reader = File::open(&config.mapping_file_path)?;
        let proto = FSVerityDigests::parse_from_reader(&mut reader)?;
        for (path_str, digest) in &proto.digests {
            if digest.hash_alg != "sha256" {
                bail!("Unsupported hash algorithm: {}", digest.hash_alg);
            }

            let file_entry = {
                let remote_path_str = path_str.strip_prefix(&config.prefix).ok_or_else(|| {
                    anyhow!("Expect path {} to match prefix {}", path_str, config.prefix)
                })?;
                AuthFsEntry::VerifiedReadonly {
                    reader: LazyVerifiedReadonlyFile::prepare_by_path(
                        service.clone(),
                        config.remote_dir_fd,
                        PathBuf::from(remote_path_str),
                        digest.digest.clone(),
                    ),
                }
            };
            authfs.add_entry_at_ro_dir_by_path(dir_root_inode, Path::new(path_str), file_entry)?;
        }
    }

    Ok(())
}

fn remote_fd_to_path_buf(fd: i32) -> PathBuf {
    PathBuf::from(fd.to_string())
}

fn try_main() -> Result<()> {
    let args = Args::parse();

    let log_level = if args.debug { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    android_logger::init_once(
        android_logger::Config::default().with_tag("authfs").with_max_level(log_level),
    );

    let service = file::get_rpc_binder_service(args.cid)?;
    let mut authfs = AuthFs::new(RemoteFsStatsReader::new(service.clone()));
    prepare_root_dir_entries(service, &mut authfs, &args)?;

    fusefs::mount_and_enter_message_loop(
        authfs,
        &args.mount_point,
        &args.extra_options,
        args.thread_number,
    )?;
    bail!("Unexpected exit after the handler loop")
}

fn main() {
    if let Err(e) = try_main() {
        error!("failed with {:?}", e);
        std::process::exit(1);
    }
}
