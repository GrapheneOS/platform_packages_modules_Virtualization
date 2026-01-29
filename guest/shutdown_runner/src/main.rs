// Copyright 2024 The Android Open Source Project
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

use api::debian_service_client::DebianServiceClient;
use api::ShutdownQueueOpeningRequest;

use clap::Parser;
use log::debug;
use shutdown_runner::power_off;
pub mod api {
    tonic::include_proto!("com.android.virtualization.terminal.proto");
}

#[derive(Parser)]
/// Flags for running command
pub struct Args {
    /// grpc port number
    #[arg(long)]
    grpc_port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder().filter_level(log::LevelFilter::Debug).init();
    let args = Args::parse();
    let gateway_ip_addr = linux::net::get_default_gateway()?;

    let grpc_port = args.grpc_port.to_string();
    let server_addr = format!("http://{}:{}", gateway_ip_addr.to_string(), grpc_port);

    debug!("connect to grpc server {}", server_addr);

    let mut client = DebianServiceClient::connect(server_addr).await.map_err(|e| e.to_string())?;

    let mut res_stream = client
        .open_shutdown_request_queue(tonic::Request::new(ShutdownQueueOpeningRequest {}))
        .await?
        .into_inner();

    while let Some(_response) = res_stream.message().await? {
        let _ = power_off();
        break;
    }
    Ok(())
}
