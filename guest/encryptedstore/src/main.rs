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

use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::Atom::{
    Atom,
    FsckFailedReported::FsckFailedReported
};
use android_system_virtualization_internal::aidl::android::system::virtualization::internal::IVmInternalService::{IVmInternalService, VM_INTERNAL_SERVICE_SOCKET_NAME};
use anyhow::{anyhow, ensure, Context, Result};
use binder::Strong;
use clap::arg;
use dm::{crypt::CipherType, util};
use log::{error, info, warn};
use rpcbinder::RpcSession;
use rustutils::system_properties;
use std::ffi::CString;
use std::fs::{self, create_dir_all, OpenOptions};
use std::io::{Error, Read, Write};
use std::os::android::fs::MetadataExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

const E2FSCK_BIN: &str = "/system/bin/e2fsck";
const MK2FS_BIN: &str = "/system/bin/mke2fs";
const RESIZE2FS_BIN: &str = "/system/bin/resize2fs";
const UNFORMATTED_STORAGE_MAGIC: &str = "UNFORMATTED-STORAGE";

static INTERNAL_CONNECTION: LazyLock<Strong<dyn IVmInternalService>> = LazyLock::new(|| {
    warn!("acquiring new connection to IVmInternalService");
    RpcSession::new().setup_unix_domain_client(VM_INTERNAL_SERVICE_SOCKET_NAME).unwrap_or_else(
        |_| panic!("Failed to connect to service: {VM_INTERNAL_SERVICE_SOCKET_NAME}"),
    )
});

// man e2fsck defines the following exit codes
#[allow(dead_code)]
#[repr(i32)]
enum FsckExitCode {
    Success = 0,
    ErrorCorrected = 1 << 0,
    SystemShouldReboot = 1 << 1,
    ErrorsLeftUncorrected = 1 << 2,
    OperationalError = 1 << 3,
    UsageOrSyntaxError = 1 << 4,
    UserCancelled = 1 << 5,
    SharedLibError = 1 << 7,
}

fn main() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("encryptedstore")
            .with_max_level(log::LevelFilter::Info),
    );

    if let Err(e) = try_main() {
        error!("{e:?}");
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
        format!("Unable to initialize encryptedstore on {blkdevice:?} & mount at {mountpoint:?}")
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

/// Gets the parent block device name from a partition name.
/// e.g., "vdb2" -> "vdb", but "dm-6" -> "dm-6".
fn get_parent_device_name(device_name: &str) -> &str {
    // Device-mapper names (dm-*) are not partitions, so return them directly.
    if device_name.starts_with("dm-") {
        return device_name;
    }

    // For other devices, check for a partition-like suffix (e.g., "vdb2").
    if let Some(last_char) = device_name.chars().last() {
        if last_char.is_ascii_digit() {
            if let Some(index) = device_name.rfind(|c: char| !c.is_ascii_digit()) {
                return &device_name[..=index];
            }
        }
    }
    // If no partition suffix is found, return the original name.
    device_name
}

/// Sets a specific tunable for a block device's queue, handling symlinks and partitions.
fn set_queue_tunable(device_path: &Path, tunable: &str, value: &str) -> Result<()> {
    // 1. Resolve the path if it's a symlink.
    let resolved_path = if fs::symlink_metadata(device_path)?.is_symlink() {
        fs::read_link(device_path)
            .with_context(|| format!("Failed to read symlink at {device_path:?}"))?
    } else {
        device_path.to_path_buf()
    };

    // 2. Get the file name (e.g., "vdb2" or "dm-6") from the resolved path.
    let resolved_device_name_str = resolved_path
        .file_name()
        .context("Could not get device name from resolved path")?
        .to_str()
        .context("Device name is not valid UTF-8")?;

    // 3. Get the parent device name (e.g., "vdb" from "vdb2").
    let parent_device_name = get_parent_device_name(resolved_device_name_str);

    // 4. Construct the final sysfs path and write the value.
    let tunable_path = format!("/sys/block/{parent_device_name}/queue/{tunable}");
    info!("Setting {parent_device_name} for {tunable}: {value}");

    if let Err(e) = fs::write(&tunable_path, value) {
        warn!("Could not write to {tunable_path}: {e}.");
    }

    Ok(())
}

fn encryptedstore_init(blkdevice: &Path, key: &str, mountpoint: &Path) -> Result<()> {
    ensure!(
        std::fs::metadata(blkdevice)
            .with_context(|| format!("Failed to get metadata of {blkdevice:?}"))?
            .file_type()
            .is_block_device(),
        "The path:{:?} is not of a block device",
        blkdevice
    );

    // Set rq_affinity for the underlying virtio-blk device (e.g., vdb)
    set_queue_tunable(blkdevice, "rq_affinity", "2")
        .context("Failed to set rq_affinity for virtio-blk device")?;

    let needs_formatting =
        needs_formatting(blkdevice).context("Unable to check if formatting is required")?;
    let crypt_device =
        enable_crypt(blkdevice, key, "cryptdev").context("Unable to map crypt device")?;

    // Set read_ahead_kb for the newly created dm-crypt device (e.g., dm-6)
    set_queue_tunable(&crypt_device, "read_ahead_kb", "512")
        .context("Failed to set read_ahead_kb for dm-crypt device")?;

    // We might need to format it with filesystem if this is a "seen-for-the-first-time" device.
    if needs_formatting {
        info!("Freshly formatting the crypt device");
        format_ext4(&crypt_device)?;
    } else {
        info!("Running e2fsck before potential resize");
        e2fsck(&crypt_device).context("e2fsck failed before potential resize")?;
        info!("Completed e2fsck before potential resize");
        info!("Running resize2fs");
        if resize_fs(&crypt_device)? {
            info!("Resized the device");
            info!("Running e2fsck after resize");
            e2fsck(&crypt_device).context("e2fsck failed after resize")?;
            info!("Completed e2fsck after resize");
        } else {
            info!("Skipped e2fsck since no resize was needed");
        }
    }

    mount(&crypt_device, mountpoint)
        .with_context(|| format!("Unable to mount {crypt_device:?}"))?;
    ensure_root_dir_status(mountpoint)?;
    if cfg!(long_running_vms) {
        system_properties::write("microdroid_manager.encrypted_store.status", "mounted")
            .context("failed to write microdroid_metadata.encryptedstore_store.status sysprop")?;
    }
    Ok(())
}

fn ensure_root_dir_status(mountpoint: &Path) -> Result<()> {
    let metadata = std::fs::metadata(mountpoint)?;
    let cur_owner = (metadata.st_uid(), metadata.st_gid());
    let want_owner = (microdroid_uids::ROOT_UID, microdroid_uids::MICRODROID_PAYLOAD_GID);
    if cur_owner != want_owner {
        warn!(
            "{mountpoint:?} owner ({cur_owner:?}) doesn't match with ({want_owner:?}). Adjusting"
        );
        nix::unistd::chown(mountpoint, Some(want_owner.0.into()), Some(want_owner.1.into()))?;
    }

    // mke2fs hardwires the root dir permissions as 0o755 which doesn't match what we want.
    // We want to allow full access by both root and the payload group, and no access by anything
    // else. And we want the sticky bit set, so different payload UIDs can create sub-directories
    // that other payloads can't delete.
    let want_mode = 0o770 | libc::S_ISVTX;
    let cur_mode = metadata.permissions().mode() & 0o7777;
    if cur_mode == want_mode {
        return Ok(());
    }
    warn!("Mode at {mountpoint:?}({cur_mode:o}) is not {want_mode:o}. Adjusting");
    std::fs::set_permissions(mountpoint, PermissionsExt::from_mode(want_mode))
        .context("Failed to chmod root directory")
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
        .opt_param("allow_discards") // This allows re-compaction of underlying disk img in host
        .opt_param("no_read_workqueue")
        .opt_param("no_write_workqueue")
        .opt_param("same_cpu_crypt")
        .opt_param("submit_from_crypt_cpus")
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
        .with_context(|| format!("Failed to open {data_device:?}"))?;

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
        .with_context(|| format!("failed to execute {MK2FS_BIN}"))?;
    ensure!(status.success(), "mkfs failed with {:?}", status);
    Ok(())
}

fn e2fsck(device: &Path) -> Result<()> {
    info!("Running e2fsck");
    let status = Command::new(E2FSCK_BIN)
        .arg("-fvy")
        .arg(device)
        .status()
        .context("failed to execute e2fsck")?;

    if status.success() {
        info!("e2fsck was successful");
        return Ok(());
    }

    info!("e2fsck wasn't successful");
    let mut exit_code = i32::MAX;
    let result = match status.code() {
        Some(code) => {
            exit_code = code;
            if code & (FsckExitCode::ErrorsLeftUncorrected as i32) != 0 {
                Err(anyhow!("File system errors left uncorrected: {code}"))
            } else {
                warn!("e2fsck exited with exitCode: {code}");
                Ok(())
            }
        }
        None => Err(anyhow!("Process terminated by signal")),
    };

    match INTERNAL_CONNECTION
        .forwardAtom(&Atom::FsckFailedReported(FsckFailedReported { exitCode: exit_code }))
    {
        Ok(()) => warn!("Wrote e2fsck exit code {exit_code} to statsd"),
        Err(e) => error!("Failed to write e2fsck exit code {exit_code} to statsd: {e}"),
    };

    result
}

/// Resizes the filesystem to the size of the device.
///
/// Returns `true` if the filesystem was resized, `false` if no resize was needed.
fn resize_fs(device: &Path) -> Result<bool> {
    info!("Running resize2fs");
    // Resize the filesystem to the size of the device.
    let output = Command::new(RESIZE2FS_BIN)
        .arg(device)
        .output()
        .context("failed to execute resize2fs")
        .unwrap();

    let stderr_str = String::from_utf8_lossy(&output.stderr);
    info!("stderr_str: {stderr_str}");
    if output.status.success() {
        info!("resize2fs command succeeded");
        let resized = !stderr_str.contains("Nothing to do!");
        info!("resized: {resized}");
        Ok(resized)
    } else {
        warn!("resize failed exited with exitCode: {stderr_str}");
        Ok(false)
    }
}

fn mount(source: &Path, mountpoint: &Path) -> Result<()> {
    create_dir_all(mountpoint).with_context(|| format!("Failed to create {:?}", &mountpoint))?;
    let mount_options = CString::new(
        "fscontext=u:object_r:encryptedstore_fs:s0,context=u:object_r:encryptedstore_file:s0,discard",
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
