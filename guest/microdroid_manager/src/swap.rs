// Copyright 2022, The Android Open Source Project
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

//! Logic for configuring and enabling a ZRAM-backed swap device.

use anyhow::{anyhow, Context, Result};
use std::fs::{read_to_string, OpenOptions};
use std::io::{Error, Seek, SeekFrom, Write};
use uuid::Uuid;

const SWAP_DEV: &str = "block/zram0";

/// Parse "MemTotal: N kB" from /proc/meminfo
fn get_total_memory_kb() -> Result<u32> {
    let s = read_to_string("/proc/meminfo")?;
    let mut iter = s.split_whitespace();
    while let Some(x) = iter.next() {
        if x.starts_with("MemTotal:") {
            let n = iter.next().context("No text after MemTotal")?;
            return n.parse::<u32>().context("No u32 after MemTotal");
        }
    }
    Err(anyhow!("MemTotal not found in /proc/meminfo"))
}

/// Simple "mkswap": Writes swap-device header into specified device.
/// The header has no formal public definition, but it can be found in the
/// Linux source tree at include/linux/swap.h (union swap_header).
/// This implementation is inspired by the one in Toybox.
fn mkswap(dev: &str) -> Result<()> {
    // Size of device, in bytes.
    let sysfs_size = format!("/sys/{}/size", dev);
    let len = read_to_string(&sysfs_size)?
        .trim()
        .parse::<u64>()
        .context(format!("No u64 in {}", &sysfs_size))?
        .checked_mul(512)
        .ok_or_else(|| anyhow!("sysfs_size too large"))?;

    // SAFETY: We give a constant and known-valid sysconf parameter.
    let pagesize = unsafe { libc::sysconf(libc::_SC_PAGE_SIZE) as u64 };

    let mut f = OpenOptions::new().read(false).write(true).open(format!("/dev/{}", dev))?;

    let last_page = len / pagesize - 1;

    // Write the info fields: [ version, last_page ]
    let info: [u32; 2] = [1, last_page.try_into().context("Number of pages out of range")?];

    f.seek(SeekFrom::Start(1024))?;
    f.write_all(&info.iter().flat_map(|v| v.to_ne_bytes()).collect::<Vec<u8>>())?;

    // Write a random version 4 UUID
    f.seek(SeekFrom::Start(1024 + 12))?;
    f.write_all(Uuid::new_v4().as_bytes())?;

    // Write the magic signature string.
    f.seek(SeekFrom::Start(pagesize - 10))?;
    f.write_all("SWAPSPACE2".as_bytes())?;

    Ok(())
}

/// Simple "swapon", using libc:: wrapper.
fn swapon(dev: &str) -> Result<()> {
    let swapon_arg = std::ffi::CString::new(format!("/dev/{}", dev))?;
    // SAFETY: We give a nul-terminated string and check the result.
    let res = unsafe { libc::swapon(swapon_arg.as_ptr(), 0) };
    if res != 0 {
        return Err(anyhow!("Failed to swapon: {}", Error::last_os_error()));
    }
    Ok(())
}

/// Turn on ZRAM-backed swap
pub fn init_swap() -> Result<()> {
    let dev = SWAP_DEV;

    // Create a ZRAM block device the same size as total VM memory.
    let mem_kb = get_total_memory_kb()?;
    OpenOptions::new()
        .read(false)
        .write(true)
        .open(format!("/sys/{}/disksize", dev))?
        .write_all(format!("{}K", mem_kb).as_bytes())?;

    mkswap(dev)?;

    swapon(dev)?;

    Ok(())
}
