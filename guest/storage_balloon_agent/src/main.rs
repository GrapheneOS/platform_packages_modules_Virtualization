// Copyright 2025 The Android Open Source Project
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

//! gRPC daemon for the storage ballooning feature.

use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use api::debian_service_client::DebianServiceClient;
use api::StorageBalloonQueueOpeningRequest;
use api::StorageBalloonRequestItem;
use clap::Parser;
use log::debug;
use log::error;
use log::info;
use nix::sys::statvfs::statvfs;
pub mod api {
    tonic::include_proto!("com.android.virtualization.terminal.proto");
}

#[derive(Parser)]
/// Flags for running command
pub struct Args {
    /// IP address
    #[arg(long)]
    addr: Option<String>,

    /// path to a file where grpc port number is written
    #[arg(long)]
    #[arg(alias = "grpc_port_file")]
    grpc_port_file: String,
}

// Calculates how many blocks to be reserved.
fn calculate_clusters_count(guest_available_bytes: u64) -> Result<u64> {
    let stat = statvfs("/").context("failed to get statvfs")?;
    let fr_size = stat.fragment_size() as u64;

    if fr_size == 0 {
        return Err(anyhow::anyhow!("fragment size is zero, fr_size: {}", fr_size));
    }

    let total = fr_size.checked_mul(stat.blocks() as u64).context(format!(
        "overflow in total size calculation, fr_size: {}, blocks: {}",
        fr_size,
        stat.blocks()
    ))?;

    let free = fr_size.checked_mul(stat.blocks_available() as u64).context(format!(
        "overflow in free size calculation, fr_size: {}, blocks_available: {}",
        fr_size,
        stat.blocks_available()
    ))?;

    let used = total
        .checked_sub(free)
        .context(format!("underflow in used size calculation (free > total), which should not happen, total: {}, free: {}", total, free))?;

    let avail = std::cmp::min(free, guest_available_bytes);
    let balloon_size_bytes = free - avail;

    let reserved_clusters_count = balloon_size_bytes.div_ceil(fr_size);

    debug!("total: {total}, free: {free}, used: {used}, avail: {avail}, balloon: {balloon_size_bytes}, clusters_count: {reserved_clusters_count}");

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder().filter_level(log::LevelFilter::Debug).init();

    let args = Args::parse();
    let gateway_ip_addr = netdev::get_default_gateway()?.ipv4[0];
    let addr = args.addr.unwrap_or_else(|| gateway_ip_addr.to_string());

    // Wait for `grpc_port_file` becomes available.
    const GRPC_PORT_MAX_RETRY_COUNT: u32 = 10;
    for _ in 0..GRPC_PORT_MAX_RETRY_COUNT {
        if std::path::Path::new(&args.grpc_port_file).exists() {
            break;
        }
        debug!("{} does not exist. Wait 1 second", args.grpc_port_file);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let grpc_port = std::fs::read_to_string(&args.grpc_port_file)?.trim().to_string();
    let server_addr = format!("http://{}:{}", addr, grpc_port);

    info!("connect to grpc server {}", server_addr);
    let mut client = DebianServiceClient::connect(server_addr)
        .await
        .map_err(|e| anyhow!("failed to connect to grpc server: {:#}", e))?;
    info!("connection established");

    let mut res_stream = client
        .open_storage_balloon_request_queue(tonic::Request::new(
            StorageBalloonQueueOpeningRequest {},
        ))
        .await
        .map_err(|e| anyhow!("failed to open storage balloon queue: {:#}", e))?
        .into_inner();

    while let Some(StorageBalloonRequestItem { available_bytes }) =
        res_stream.message().await.map_err(|e| anyhow!("failed to receive message: {:#}", e))?
    {
        let clusters_count = match calculate_clusters_count(available_bytes) {
            Ok(c) => c,
            Err(e) => {
                error!("failed to calculate cluster size to be reserved: {:#}", e);
                continue;
            }
        };

        if let Err(e) = set_reserved_clusters(clusters_count) {
            error!("failed to set storage balloon size: {}", e);
        }
    }

    Ok(())
}
