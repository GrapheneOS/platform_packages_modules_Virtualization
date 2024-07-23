/*
 * Copyright (C) 2022 The Android Open Source Project
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

//! `encryptedstore` is a program that (as the name indicates) provides encrypted storage
//! solution in a VM. This is based on dm-crypt & requires the (64 bytes') key & the backing device.
//! It uses dm_rust lib.

use anyhow::{ensure, Context, Result};
use clap::arg;
use dm::{crypt::CipherType, util};
use log::{error, info};
use std::ffi::CString;
use std::fs::{create_dir_all, OpenOptions};
use std::io::{Error, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

const MK2FS_BIN: &str = "/system/bin/mke2fs";
const UNFORMATTED_STORAGE_MAGIC: &str = "UNFORMATTED-STORAGE";

fn main() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("encryptedstore")
            .with_max_level(log::LevelFilter::Info),
    );

    if let Err(e) = try_main() {
        error!("{:?}", e);
        std::process::exit(1)
    }
}

fn try_main() -> Result<()> {
    info!("Starting encryptedstore binary");

    let matches = clap_command().get_matches();

    let blkdevice = Path::new(matches.get_one::<String>("blkdevice").unwrap());
    let key = matches.get_one::<String>("key").unwrap();
    let mountpoint = Path::new(matches.get_one::<String>("mountpoint").unwrap());
    // Note this error context is used in MicrodroidTests.
    encryptedstore_init(blkdevice, key, mountpoint).with_context(|| {
        format!(
            "Unable to initialize encryptedstore on {:?} & mount at {:?}",
            blkdevice, mountpoint
        )
    })?;
    Ok(())
}

fn clap_command() -> clap::Command {
    clap::Command::new("encryptedstore").args(&[
        arg!(--blkdevice <FILE> "the block device backing the encrypted storage").required(true),
        arg!(--key <KEY> "key (in hex) equivalent to 32 bytes)").required(true),
        arg!(--mountpoint <MOUNTPOINT> "mount point for the storage").required(true),
    ])
}

fn encryptedstore_init(blkdevice: &Path, key: &str, mountpoint: &Path) -> Result<()> {
    ensure!(
        std::fs::metadata(blkdevice)
            .with_context(|| format!("Failed to get metadata of {:?}", blkdevice))?
            .file_type()
            .is_block_device(),
        "The path:{:?} is not of a block device",
        blkdevice
    );

    let needs_formatting =
        needs_formatting(blkdevice).context("Unable to check if formatting is required")?;
    let crypt_device =
        enable_crypt(blkdevice, key, "cryptdev").context("Unable to map crypt device")?;

    // We might need to format it with filesystem if this is a "seen-for-the-first-time" device.
    if needs_formatting {
        info!("Freshly formatting the crypt device");
        format_ext4(&crypt_device)?;
    }
    mount(&crypt_device, mountpoint)
        .with_context(|| format!("Unable to mount {:?}", crypt_device))?;
    if cfg!(multi_tenant) && needs_formatting {
        set_root_dir_permissions(mountpoint)?;
    }
    Ok(())
}

fn set_root_dir_permissions(mountpoint: &Path) -> Result<()> {
    // mke2fs hardwires the root dir permissions as 0o755 which doesn't match what we want.
    // We want to allow full access by both root and the payload group, and no access by anything
    // else. And we want the sticky bit set, so different payload UIDs can create sub-directories
    // that other payloads can't delete.
    let permissions = PermissionsExt::from_mode(0o770 | libc::S_ISVTX);
    std::fs::set_permissions(mountpoint, permissions).context("Failed to chmod root directory")
}

fn enable_crypt(data_device: &Path, key: &str, name: &str) -> Result<PathBuf> {
    let dev_size = util::blkgetsize64(data_device)?;
    let key = hex::decode(key).context("Unable to decode hex key")?;

    // Create the dm-crypt spec
    let target = dm::crypt::DmCryptTargetBuilder::default()
        .data_device(data_device, dev_size)
        .cipher(CipherType::AES256HCTR2)
        .key(&key)
        .opt_param("sector_size:4096")
        .opt_param("iv_large_sectors")
        .build()
        .context("Couldn't build the DMCrypt target")?;
    let dm = dm::DeviceMapper::new()?;
    dm.create_crypt_device(name, &target).context("Failed to create dm-crypt device")
}

// The disk contains UNFORMATTED_STORAGE_MAGIC to indicate we need to format the crypt device.
// This function looks for it, zeroing it, if present.
fn needs_formatting(data_device: &Path) -> Result<bool> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(data_device)
        .with_context(|| format!("Failed to open {:?}", data_device))?;

    let mut buf = [0; UNFORMATTED_STORAGE_MAGIC.len()];
    file.read_exact(&mut buf)?;

    if buf == UNFORMATTED_STORAGE_MAGIC.as_bytes() {
        buf.fill(0);
        file.write_all(&buf)?;
        return Ok(true);
    }
    Ok(false)
}

fn format_ext4(device: &Path) -> Result<()> {
    let root_dir_uid_gid = format!(
        "root_owner={}:{}",
        microdroid_uids::ROOT_UID,
        microdroid_uids::MICRODROID_PAYLOAD_GID
    );
    let mkfs_options = [
        "-j", // Create appropriate sized journal
        /* metadata_csum: enabled for filesystem integrity
         * extents: Not enabling extents reduces the coverage of metadata checksumming.
         * 64bit: larger fields afforded by this feature enable full-strength checksumming.
         */
        "-O metadata_csum, extents, 64bit",
        "-b 4096", // block size in the filesystem,
        "-E",
        &root_dir_uid_gid,
    ];
    let mut cmd = Command::new(MK2FS_BIN);
    let status = cmd
        .args(mkfs_options)
        .arg(device)
        .status()
        .with_context(|| format!("failed to execute {}", MK2FS_BIN))?;
    ensure!(status.success(), "mkfs failed with {:?}", status);
    Ok(())
}

fn mount(source: &Path, mountpoint: &Path) -> Result<()> {
    create_dir_all(mountpoint).with_context(|| format!("Failed to create {:?}", &mountpoint))?;
    let mount_options = CString::new(
        "fscontext=u:object_r:encryptedstore_fs:s0,context=u:object_r:encryptedstore_file:s0",
    )
    .unwrap();
    let source = CString::new(source.as_os_str().as_bytes())?;
    let mountpoint = CString::new(mountpoint.as_os_str().as_bytes())?;
    let fstype = CString::new("ext4").unwrap();

    // SAFETY: The source, target and filesystemtype are valid C strings. For ext4, data is expected
    // to be a C string as well, which it is. None of these pointers are retained after mount
    // returns.
    let ret = unsafe {
        libc::mount(
            source.as_ptr(),
            mountpoint.as_ptr(),
            fstype.as_ptr(),
            libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
            mount_options.as_ptr() as *const std::ffi::c_void,
        )
    };
    if ret < 0 {
        Err(Error::last_os_error()).context("mount failed")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_command() {
        // Check that the command parsing has been configured in a valid way.
        clap_command().debug_assert();
    }
}
