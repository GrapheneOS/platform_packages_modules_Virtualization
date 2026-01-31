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
use anyhow::Result;
use api::debian_service_client::DebianServiceClient;
use api::StorageBalloonQueueOpeningRequest;
use api::StorageBalloonRequestItem;
use clap::Parser;
use log::debug;
use log::error;
use log::info;
use storage_balloon_agent::do_storage_ballooning;
pub mod api {
    tonic::include_proto!("com.android.virtualization.terminal.proto");
}

#[derive(Parser)]
/// Flags for running command
pub struct Args {
    /// IP address
    #[arg(long)]
    addr: Option<String>,

    /// grpc port number
    #[arg(long)]
    grpc_port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder().filter_level(log::LevelFilter::Debug).init();

    let args = Args::parse();
    let gateway_ip_addr = linux::net::get_default_gateway()?;
    let addr = args.addr.unwrap_or_else(|| gateway_ip_addr.to_string());

    let grpc_port = args.grpc_port.to_string();
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
        if let Err(e) = do_storage_ballooning(available_bytes) {
            error!("Failed to do storage ballooning. {e:?}");
        }
    }

    Ok(())
}
