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

use android_system_virtualization_payload::aidl::android::system::virtualization::payload::IVmPayloadService::VM_APK_CONTENTS_PATH;
use anyhow::{bail, ensure, Context, Result};
use dm::{
    crypt::{CipherType, DmCryptTargetBuilder},
    loopdevice::{attach as loop_attach, detach as loop_detach},
    DeviceMapper,
};
use nix::mount::{mount, MsFlags};
use rustutils::android::system_properties;
use scopeguard::{defer, guard, ScopeGuard};
use std::{
    fs,
    path::{Path, PathBuf},
    thread::sleep,
    time::{Duration, Instant},
};
use thiserror::Error;

const UEVENTD_SERVICE: &str = "ueventd";
const UEVENTD_STATUS_PROP: &str = "init.svc.ueventd";
const DM_CRYPT_DEVICE_NAME: &str = "encrypted_asset";
const MOUNT_POINT: &str = "/mnt/encrypted_assets";
const SELINUX_CONTEXT: &str = "fscontext=u:object_r:encrypted_assets_fs:s0,\
                               context=u:object_r:encrypted_assets_file:s0";

const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Error, Debug)]
pub enum MountError {
    #[error("Bad image")]
    BadImage,

    #[error("Bad filesystem type")]
    BadFsType,

    #[error("Bad cipher")]
    BadCipher,

    #[error("Bad key size")]
    BadKeySize,

    #[error("Bad sector size")]
    BadSectorSize,

    #[error("Internal error")]
    Other,
}

/// Creates a dm-crypt mapping over an encrypted image file and mounts the contained filesystem.
///
/// TODO(b/455757575): Extract this logic to a separate binary (similar to encryptedstore) to
/// reduce the privileges required by microdroid_manager.
pub fn mount_encrypted_assets(
    image_path: &str,
    fs_type: &str,
    cipher: &str,
    key: &[u8],
    sector_size: i32,
) -> Result<String> {
    let (image_path, image_size) = resolve_image(image_path).context(MountError::BadImage)?;
    validate_fs_type(fs_type).context(MountError::BadFsType)?;
    let cipher = resolve_cipher(cipher).context(MountError::BadCipher)?;
    validate_key_size(&cipher, key).context(MountError::BadKeySize)?;
    validate_sector_size(sector_size).context(MountError::BadSectorSize)?;
    validate_image_size(sector_size, image_size).context(MountError::BadImage)?;

    // Device mapper requires ueventd to handle kernel uevents and create the corresponding
    // /dev/mapper device nodes. Since ueventd is stopped after boot to minimize memory usage,
    // we need to start it on demand for the duration of the mount operation.
    start_ueventd().context(MountError::Other)?;

    // Ensure resources are cleaned up when this function exits, even if errors occur.
    // We panic if any cleanup operation fails, as this indicates a serious issue and helps prevent
    // leaving the system in an inconsistent or insecure state.
    defer! { stop_ueventd().unwrap(); };

    let loop_path = setup_loop(&image_path, image_size)?;
    let loop_guard = guard((), |_| {
        loop_detach(&loop_path).unwrap();
    });

    let dm_path = setup_dm_crypt(&loop_path, cipher, key, sector_size, image_size)
        .context(MountError::Other)?;
    let dm_guard = guard((), |_| {
        DeviceMapper::new().unwrap().delete_device_deferred(DM_CRYPT_DEVICE_NAME).unwrap();
    });

    do_mount(&dm_path, fs_type).context(MountError::Other)?;

    // Defuse the mount related guards if everything went well.
    ScopeGuard::into_inner(loop_guard);
    ScopeGuard::into_inner(dm_guard);

    Ok(MOUNT_POINT.to_owned())
}

/// Resolves the image path to its canonical form with its size.
fn resolve_image(path: &str) -> Result<(PathBuf, u64)> {
    let image_path = Path::new(path).canonicalize().context("Failed to canonicalize image path")?;

    // Ensure the image path is located within the APK's assets directory.
    // This guarantees the image is read-only and its integrity has been verified by the APK
    // signature.
    let assets_prefix = Path::new(VM_APK_CONTENTS_PATH).join("assets");
    ensure!(image_path.starts_with(&assets_prefix), "Invalid image path: {image_path:?}");

    let image_size = fs::metadata(&image_path).context("Failed to get image size")?.len();
    ensure!(image_size > 0, "Image is empty");

    Ok((image_path, image_size))
}

/// Resolves the cipher name to its corresponding CipherType.
fn resolve_cipher(name: &str) -> Result<CipherType> {
    match name {
        "aes-hctr2-plain64" => Ok(CipherType::AES256HCTR2),
        "aes-xts-plain64" => Ok(CipherType::AES256XTS),
        other => bail!("Unsupported cipher: {other}"),
    }
}

/// Validates the key size matches the expected size for the given cipher.
fn validate_key_size(cipher: &CipherType, key: &[u8]) -> Result<()> {
    let expected = cipher.get_required_key_size();
    ensure!(expected == key.len(), "Invalid key size: expected {expected}, got {}", key.len());
    Ok(())
}

/// Validates the filesystem type for mounting encrypted assets is a known, read-only filesystem.
fn validate_fs_type(fs_type: &str) -> Result<()> {
    match fs_type {
        "erofs" => Ok(()),
        _ => bail!("Unsupported fs_type: {fs_type}"),
    }
}

/// Validates the sector size is a power of two within the valid range [512, 4096].
///
/// Ref: https://docs.kernel.org/admin-guide/device-mapper/dm-crypt.html
fn validate_sector_size(sector_size: i32) -> Result<()> {
    ensure!(
        (512..=4096).contains(&sector_size) && (sector_size as u32).is_power_of_two(),
        "Invalid sector size: {sector_size}"
    );
    Ok(())
}

/// Validates the image size is a multiple of the sector size.
fn validate_image_size(sector_size: i32, image_size: u64) -> Result<()> {
    ensure!(
        image_size.is_multiple_of(sector_size as u64),
        "Image size {image_size} is not a multiple of sector size {sector_size}"
    );
    Ok(())
}

/// Waits for ueventd to reach the given status within the timeout duration.
fn wait_ueventd_status(value: &str) -> Result<()> {
    system_properties::PropertyWatcher::new(UEVENTD_STATUS_PROP)
        .context("Failed to create PropertyWatcher for ueventd")?
        .wait_for_value(value, Some(TIMEOUT))
        .with_context(|| format!("Failed to wait for ueventd to be {value}"))
}

/// Starts ueventd and waits until it is ready to handle uevents.
fn start_ueventd() -> Result<()> {
    // To avoid race conditions, we must wait for ueventd to fully stop before attempting to restart
    // it. A signal to stop might have been sent by init but not yet processed.
    wait_ueventd_status("stopped")?;

    // Starts ueventd and waits for it to be running.
    system_properties::write("ctl.start", UEVENTD_SERVICE).context("Failed to start ueventd")?;
    wait_ueventd_status("running")?;

    // There is a brief window after ueventd starts but before it begins listening for uevents.
    // On a Cuttlefish VM, this gap is roughly 1ms. We poll /proc/net/netlink to ensure it's ready.
    const INTERVAL: Duration = Duration::from_millis(1);
    let begin = Instant::now();
    loop {
        let netlink_data = fs::read_to_string("/proc/net/netlink")?;
        if is_ueventd_listener_present(&netlink_data) {
            break;
        }
        if begin.elapsed() >= TIMEOUT {
            bail!("ueventd is not ready within {TIMEOUT:?}");
        }
        sleep(INTERVAL);
    }
    Ok(())
}

/// Checks if ueventd is listening for uevents by parsing /proc/net/netlink content.
///
/// TODO(b/455543595): Have a better ueventd readiness signal to eliminate procfs parsing.
fn is_ueventd_listener_present(netlink_data: &str) -> bool {
    // Defined in linux kernel include/uapi/linux/netlink.h.
    const NETLINK_KOBJECT_UEVENT: u32 = 15;

    // Here is an example of /proc/net/netlink content when ueventd is listening:
    // sk               Eth Pid        Groups   Rmem     Wmem     Dump  Locks    Drops    Inode
    // 0000000000000000 0   0          00000000 0        0        0     2        0        3
    // 0000000000000000 10  0          00000000 0        0        0     2        0        93
    // 0000000000000000 15  0          00000000 0        0        0     2        0        9
    // 0000000000000000 15  117        ffffffff 0        0        0     2        0        1743
    // 0000000000000000 16  0          00000000 0        0        0     2        0        4
    //
    // We look for a line with:
    // * Eth=15 (NETLINK_KOBJECT_UEVENT), and
    // * Groups=0xffffffff (all multicast groups)
    // to determine if ueventd is listening.
    //
    // Ref: netlink_native_seq_show() in net/netlink/af_netlink.c.
    netlink_data
        .lines()
        .skip(1) // Skips header line
        .any(|line| {
            let mut cols = line.split_whitespace();

            // Skips sk column and parses Eth column.
            let eth = cols.nth(1).and_then(|s| s.parse::<u32>().ok());

            // Skips Pid column and parses Groups column.
            let groups = cols.nth(1).and_then(|s| u32::from_str_radix(s, 16).ok());

            eth == Some(NETLINK_KOBJECT_UEVENT) && groups == Some(0xffffffff)
        })
}

/// Stops ueventd and waits until it is stopped.
fn stop_ueventd() -> Result<()> {
    system_properties::write("ctl.stop", UEVENTD_SERVICE).context("Failed to stop ueventd")?;
    wait_ueventd_status("stopped")
}

/// Sets up a loop device for the given image. Returns the loop device path.
fn setup_loop(image: &Path, size: u64) -> Result<PathBuf> {
    let loop_dev = loop_attach(image, /* offset= */ 0, size, &Default::default())
        .context("Failed to attach loop device")?;
    Ok(loop_dev.path)
}

/// Sets up a dm-crypt device on top of the given loop device. Returns the dm-crypt device path.
fn setup_dm_crypt(
    loop_path: &Path,
    cipher: CipherType,
    key: &[u8],
    sector_size: i32,
    image_size: u64,
) -> Result<PathBuf> {
    let target = DmCryptTargetBuilder::default()
        .cipher(cipher)
        .key(key)
        .data_device(loop_path, image_size)
        .opt_param("iv_large_sectors")
        .opt_param(&format!("sector_size:{sector_size}"))
        .build()
        .context("Failed to build dm-crypt target")?;

    let dm = DeviceMapper::new().context("Failed to create DeviceMapper")?;
    dm.create_crypt_device(DM_CRYPT_DEVICE_NAME, &target)
        .context("Failed to create dm-crypt device")
}

/// Mounts the dm-crypt device at the predefined mount point with the given filesystem type.
fn do_mount(dm_path: &Path, fs_type: &str) -> Result<()> {
    fs::create_dir_all(MOUNT_POINT).context("Failed to create mount point")?;

    mount(
        Some(dm_path),
        Path::new(MOUNT_POINT),
        Some(fs_type),
        MsFlags::MS_RDONLY | MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
        Some(SELINUX_CONTEXT),
    )
    .context("Failed to mount encrypted asset")
}

#[cfg(test)]
mod test {
    use super::*;
    use itertools::Itertools;

    macro_rules! dedent {
        ($s:expr) => {
            do_dedent($s)
        };
    }

    fn do_dedent(s: &str) -> String {
        let level = s
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.bytes().take_while(|&c| c == b' ').count())
            .min()
            .unwrap_or(0);
        s.lines()
            .skip_while(|line| line.is_empty())
            .map(|line| if line.len() >= level { &line[level..] } else { line })
            .join("\n")
    }

    #[test]
    fn test_validate_sector_size() {
        assert!(validate_sector_size(512).is_ok());
        assert!(validate_sector_size(1024).is_ok());
        assert!(validate_sector_size(4096).is_ok());

        assert!(validate_sector_size(256).is_err()); // Too small
        assert!(validate_sector_size(1000).is_err()); // Not power of 2
        assert!(validate_sector_size(8192).is_err()); // Too large
    }

    #[test]
    fn ueventd_is_ready_with_correct_eth_and_groups() {
        let data = dedent! {"
            sk               Eth Pid        Groups   Rmem     Wmem     Dump  Locks    Drops    Inode
            0000000000000000 0   0          00000000 0        0        0     2        0        3
            0000000000000000 10  0          00000000 0        0        0     2        0        93
            0000000000000000 15  0          00000000 0        0        0     2        0        9
            0000000000000000 15  117        ffffffff 0        0        0     2        0        1743
            0000000000000000 16  0          00000000 0        0        0     2        0        4
        "};
        assert!(is_ueventd_listener_present(&data));
    }

    #[test]
    fn ueventd_is_not_ready_with_incorrect_eth() {
        let data = dedent! {"
            sk               Eth Pid        Groups   Rmem     Wmem     Dump  Locks    Drops    Inode
            0000000000000000 0   0          00000000 0        0        0     2        0        3
            0000000000000000 10  0          00000000 0        0        0     2        0        93
            0000000000000000 15  0          00000000 0        0        0     2        0        9
            0000000000000000 16  117        ffffffff 0        0        0     2        0        1743
            0000000000000000 16  0          00000000 0        0        0     2        0        4
        "};
        assert!(!is_ueventd_listener_present(&data));
    }

    #[test]
    fn ueventd_is_not_ready_with_incorrect_groups() {
        let data = dedent! {"
            sk               Eth Pid        Groups   Rmem     Wmem     Dump  Locks    Drops    Inode
            0000000000000000 0   0          00000000 0        0        0     2        0        3
            0000000000000000 10  0          00000000 0        0        0     2        0        93
            0000000000000000 15  0          00000000 0        0        0     2        0        9
            0000000000000000 15  117        deadbeef 0        0        0     2        0        1743
            0000000000000000 16  0          00000000 0        0        0     2        0        4
        "};
        assert!(!is_ueventd_listener_present(&data));
    }
}
