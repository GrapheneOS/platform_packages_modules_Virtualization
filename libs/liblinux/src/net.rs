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

//! Libraries for Linux network

use anyhow::{anyhow, Context, Error};
use std::fs::File;
use std::io::{self, BufRead};
use std::net::Ipv4Addr;

/// Get default gateway in IPV4 as string by reading /proc/net/route
pub fn get_default_gateway() -> Result<String, Error> {
    const DEFAULT_ROUTE_DESTINATION: &str = "00000000";

    // /proc/net/route isn't an ordinary file, so better not use tokio.
    let file = File::open("/proc/net/route")?;
    let mut lines = io::BufReader::new(file).lines();

    let header_line = lines.next().ok_or(anyhow!("Cannot find header from /proc/net/route"))??;
    let headers: Vec<_> = header_line.split_whitespace().collect();
    let dest_idx = headers
        .iter()
        .position(|col| *col == "Destination")
        .ok_or(anyhow!("Cannot find 'Destination' from header"))?;
    let gateway_idx = headers
        .iter()
        .position(|col| *col == "Gateway")
        .ok_or(anyhow!("Cannot find 'Gateway' from header"))?;

    while let Some(Ok(line)) = lines.next() {
        let items: Vec<_> = line.split_whitespace().collect();
        if items[dest_idx] == DEFAULT_ROUTE_DESTINATION {
            let ip_as_int = u32::from_str_radix(items[gateway_idx], 16)
                .context("Cannot convert gateway address. Unexpected address format")?;
            return Ok(Ipv4Addr::from_bits(ip_as_int).to_string());
        }
    }
    Err(anyhow!("Cannot find default gateway"))
}
