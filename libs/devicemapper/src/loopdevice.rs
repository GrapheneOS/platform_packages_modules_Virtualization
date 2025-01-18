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

// `loopdevice` module provides `attach` and `detach` functions that are for attaching and
// detaching a regular file to and from a loop device. Note that
// `loopdev`(https://crates.io/crates/loopdev) is a public alternative to this. In-house
// implementation was chosen to make Android-specific changes (like the use of the new
// LOOP_CONFIGURE instead of the legacy LOOP_SET_FD + LOOP_SET_STATUS64 combo which is considerably
// slower than the former).

mod sys;

use crate::util::*;
use anyhow::{Context, Result};
use libc::O_DIRECT;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use zerocopy::FromZeros;

use crate::loopdevice::sys::*;

// These are old-style ioctls, thus *_bad.
nix::ioctl_none_bad!(_loop_ctl_get_free, LOOP_CTL_GET_FREE);
nix::ioctl_write_ptr_bad!(_loop_configure, LOOP_CONFIGURE, loop_config);
nix::ioctl_none_bad!(_loop_clr_fd, LOOP_CLR_FD);

fn loop_ctl_get_free(ctrl_file: &File) -> Result<i32> {
    // SAFETY: this ioctl changes the state in kernel, but not the state in this process.
    // The returned device number is a global resource; not tied to this process. So, we don't
    // need to keep track of it.
    Ok(unsafe { _loop_ctl_get_free(ctrl_file.as_raw_fd()) }?)
}

fn loop_configure(device_file: &File, config: &loop_config) -> Result<i32> {
    // SAFETY: this ioctl changes the state in kernel, but not the state in this process.
    Ok(unsafe { _loop_configure(device_file.as_raw_fd(), config) }?)
}

pub fn loop_clr_fd(device_file: &File) -> Result<i32> {
    // SAFETY: this ioctl disassociates the loop device with `device_file`, where the FD will
    // remain opened afterward. The association itself is kept for open FDs.
    Ok(unsafe { _loop_clr_fd(device_file.as_raw_fd()) }?)
}

/// LOOP_CONFIGURE ioctl operation flags.
#[derive(Default)]
pub struct LoopConfigOptions {
    /// Whether to use direct I/O
    pub direct_io: bool,
    /// Whether the device is writable
    pub writable: bool,
    /// Whether to autodestruct the device on last close
    pub autoclear: bool,
}

pub struct LoopDevice {
    /// The loop device file
    pub file: File,
    /// Path to the loop device
    pub path: PathBuf,
}

/// Creates a loop device and attach the given file at `path` as the backing store.
pub fn attach<P: AsRef<Path>>(
    path: P,
    offset: u64,
    size_limit: u64,
    options: &LoopConfigOptions,
) -> Result<LoopDevice> {
    // Attaching a file to a loop device can make a race condition; a loop device number obtained
    // from LOOP_CTL_GET_FREE might have been used by another thread or process. In that case the
    // subsequent LOOP_CONFIGURE ioctl returns with EBUSY. Try until it succeeds.
    //
    // Note that the timing parameters below are chosen rather arbitrarily. In practice (i.e.
    // inside Microdroid) we can't experience the race condition because `apkverity` is the only
    // user of /dev/loop-control at the moment. This loop is mostly for testing where multiple
    // tests run concurrently.
    const TIMEOUT: Duration = Duration::from_secs(1);
    const INTERVAL: Duration = Duration::from_millis(10);

    let begin = Instant::now();
    loop {
        match try_attach(&path, offset, size_limit, options) {
            Ok(loop_device) => return Ok(loop_device),
            Err(e) => {
                if begin.elapsed() > TIMEOUT {
                    return Err(e);
                }
            }
        };
        thread::sleep(INTERVAL);
    }
}

#[cfg(not(target_os = "android"))]
const LOOP_DEV_PREFIX: &str = "/dev/loop";

#[cfg(target_os = "android")]
const LOOP_DEV_PREFIX: &str = "/dev/block/loop";

fn try_attach<P: AsRef<Path>>(
    path: P,
    offset: u64,
    size_limit: u64,
    options: &LoopConfigOptions,
) -> Result<LoopDevice> {
    // Get a free loop device
    wait_for_path(LOOP_CONTROL)?;
    let ctrl_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(LOOP_CONTROL)
        .context("Failed to open loop control")?;
    let num = loop_ctl_get_free(&ctrl_file).context("Failed to get free loop device")?;

    // Construct the loop_info64 struct
    let backing_file = OpenOptions::new()
        .read(true)
        .write(options.writable)
        .custom_flags(if options.direct_io { O_DIRECT } else { 0 })
        .open(&path)
        .context(format!("failed to open {:?}", path.as_ref()))?;
    let mut config = loop_config::new_zeroed();
    config.fd = backing_file.as_raw_fd() as u32;
    config.block_size = 4096;
    config.info.lo_offset = offset;
    config.info.lo_sizelimit = size_limit;

    if !options.writable {
        config.info.lo_flags = Flag::LO_FLAGS_READ_ONLY;
    }

    if options.direct_io {
        config.info.lo_flags.insert(Flag::LO_FLAGS_DIRECT_IO);
    }

    if options.autoclear {
        config.info.lo_flags.insert(Flag::LO_FLAGS_AUTOCLEAR);
    }

    // Configure the loop device to attach the backing file
    let device_path = format!("{}{}", LOOP_DEV_PREFIX, num);
    wait_for_path(&device_path)?;
    let device_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&device_path)
        .context(format!("failed to open {:?}", &device_path))?;
    loop_configure(&device_file, &config)
        .context(format!("Failed to configure {:?}", &device_path))?;

    Ok(LoopDevice { file: device_file, path: PathBuf::from(device_path) })
}

/// Detaches backing file from the loop device `path`.
pub fn detach<P: AsRef<Path>>(path: P) -> Result<()> {
    let device_file = OpenOptions::new().read(true).write(true).open(&path)?;
    loop_clr_fd(&device_file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdroidtest::rdroidtest;
    use std::fs;
    use std::path::Path;

    fn create_empty_file(path: &Path, size: u64) {
        let f = File::create(path).unwrap();
        f.set_len(size).unwrap();
    }

    fn is_direct_io(dev: &Path) -> bool {
        let dio = Path::new("/sys/block").join(dev.file_name().unwrap()).join("loop/dio");
        "1" == fs::read_to_string(dio).unwrap().trim()
    }

    // kernel exposes /sys/block/loop*/ro which gives the read-only value
    fn is_direct_io_writable(dev: &Path) -> bool {
        let ro = Path::new("/sys/block").join(dev.file_name().unwrap()).join("ro");
        "0" == fs::read_to_string(ro).unwrap().trim()
    }

    #[rdroidtest]
    fn attach_loop_device_with_dio() {
        let a_dir = tempfile::TempDir::new().unwrap();
        let a_file = a_dir.path().join("test");
        let a_size = 4096u64;
        create_empty_file(&a_file, a_size);
        let dev =
            attach(a_file, 0, a_size, &LoopConfigOptions { direct_io: true, ..Default::default() })
                .unwrap()
                .path;
        scopeguard::defer! {
            detach(&dev).unwrap();
        }
        assert!(is_direct_io(&dev));
    }

    #[rdroidtest]
    fn attach_loop_device_without_dio() {
        let a_dir = tempfile::TempDir::new().unwrap();
        let a_file = a_dir.path().join("test");
        let a_size = 4096u64;
        create_empty_file(&a_file, a_size);
        let dev = attach(a_file, 0, a_size, &LoopConfigOptions::default()).unwrap().path;
        scopeguard::defer! {
            detach(&dev).unwrap();
        }
        assert!(!is_direct_io(&dev));
    }

    #[rdroidtest]
    fn attach_loop_device_with_dio_writable() {
        let a_dir = tempfile::TempDir::new().unwrap();
        let a_file = a_dir.path().join("test");
        let a_size = 4096u64;
        create_empty_file(&a_file, a_size);
        let dev = attach(
            a_file,
            0,
            a_size,
            &LoopConfigOptions { direct_io: true, writable: true, ..Default::default() },
        )
        .unwrap()
        .path;
        scopeguard::defer! {
            detach(&dev).unwrap();
        }
        assert!(is_direct_io(&dev));
        assert!(is_direct_io_writable(&dev));
    }

    #[rdroidtest]
    fn attach_loop_device_autoclear() {
        let a_dir = tempfile::TempDir::new().unwrap();
        let a_file = a_dir.path().join("test");
        let a_size = 4096u64;
        create_empty_file(&a_file, a_size);
        let dev =
            attach(a_file, 0, a_size, &LoopConfigOptions { autoclear: true, ..Default::default() })
                .unwrap();
        drop(dev.file);

        let dev_size_path =
            Path::new("/sys/block").join(dev.path.file_name().unwrap()).join("size");
        assert_eq!("0", fs::read_to_string(dev_size_path).unwrap().trim());
    }
}
