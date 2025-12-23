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

//! A library that handles storage ballooning

use anyhow::{anyhow, Context, Result};
use log::debug;
use nix::sys::statvfs::statvfs;

/// Do storage ballooning based on the available bytes.
pub fn do_storage_ballooning(available_bytes: u64) -> Result<()> {
    let clusters_count = calculate_clusters_count(available_bytes)
        .map_err(|e| anyhow!("failed to calculate cluster size to be reserved: {:#}", e))?;

    set_reserved_clusters(clusters_count)
        .map_err(|e| anyhow!("failed to set storage balloon size: {}", e))
}

// Calculates how many blocks to be reserved.
#[allow(clippy::useless_conversion)]
fn calculate_clusters_count(guest_available_bytes: u64) -> Result<u64> {
    let stat = statvfs("/").context("failed to get statvfs")?;
    let fr_size: u64 = stat.fragment_size().into();

    if fr_size == 0 {
        return Err(anyhow::anyhow!("fragment size is zero, fr_size: {}", fr_size));
    }

    let total = fr_size.checked_mul(stat.blocks().into()).context(format!(
        "overflow in total size calculation, fr_size: {}, blocks: {}",
        fr_size,
        stat.blocks()
    ))?;

    let free = fr_size.checked_mul(stat.blocks_available().into()).context(format!(
        "overflow in free size calculation, fr_size: {}, blocks_available: {}",
        fr_size,
        stat.blocks_available()
    ))?;

    let current_reserved_clusters_count = get_reserved_clusters()?;
    let current_reserved_clusters_size =
        current_reserved_clusters_count.checked_mul(fr_size).context(format!(
            "overflow in calculate_reserved_clusters_size calculation,
            current_reserved_clusters_count: {}, fr_size: {}",
            current_reserved_clusters_count, fr_size
        ))?;

    let used = total.checked_sub(free + current_reserved_clusters_size).context(format!(
        "underflow in used size calculation (free + current_reserved_clusters_size > total), which
        should not happen, total: {}, free: {}, current_reserved_clusters_size: {}",
        total, free, current_reserved_clusters_size
    ))?;

    let mut balloon_size_bytes = 0_u64;
    if total > guest_available_bytes + used {
        balloon_size_bytes = total - guest_available_bytes - used;
    }

    let reserved_clusters_count = balloon_size_bytes.div_ceil(fr_size);

    debug!("total: {total}, free: {free}, used: {used}, guest_avail: {guest_available_bytes}, balloon: {balloon_size_bytes}, clusters_count: {reserved_clusters_count}");

    Ok(reserved_clusters_count)
}

fn set_reserved_clusters(clusters_count: u64) -> anyhow::Result<()> {
    const ROOTFS_DEVICE_NAME: &str = "vda1";
    std::fs::write(
        format!("/sys/fs/ext4/{ROOTFS_DEVICE_NAME}/reserved_clusters"),
        clusters_count.to_string(),
    )
    .context("failed to write reserved_clusters")?;
    Ok(())
}

fn get_reserved_clusters() -> anyhow::Result<u64> {
    const ROOTFS_DEVICE_NAME: &str = "vda1";
    let path = format!("/sys/fs/ext4/{ROOTFS_DEVICE_NAME}/reserved_clusters");
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("failed to read from {path}"))?;
    let clusters_count = content
        .trim()
        .parse::<u64>()
        .with_context(|| format!("failed to parse content of {path}: '{content}'"))?;
    Ok(clusters_count)
}
