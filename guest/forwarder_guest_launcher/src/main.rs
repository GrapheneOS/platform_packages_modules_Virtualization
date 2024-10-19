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

//! Launcher of forwarder_guest

use clap::Parser;
use debian_service::debian_service_client::DebianServiceClient;
use debian_service::Empty;
use tonic::transport::Endpoint;
use tonic::Request;

mod debian_service {
    tonic::include_proto!("com.android.virtualization.vmlauncher.proto");
}

#[derive(Parser)]
/// Flags for running command
pub struct Args {
    /// Host IP address
    #[arg(long)]
    #[arg(alias = "host")]
    host_addr: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let addr = format!("https://{}:12000", args.host_addr);

    let channel = Endpoint::from_shared(addr)?.connect().await?;
    let mut client = DebianServiceClient::new(channel);
    let mut res_stream =
        client.open_forwarding_request_queue(Request::new(Empty {})).await?.into_inner();

    while let Some(response) = res_stream.message().await? {
        println!("Response from the host: {:?}", response);
    }
    Ok(())
}
