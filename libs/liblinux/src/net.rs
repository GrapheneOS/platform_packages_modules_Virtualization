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
            let ip_be = u32::from_str_radix(items[gateway_idx], 16)
                .context("Cannot convert gateway address. Unexpected address format")?;
            // Kernel keeps the IP address as big endian while Ipv4Addr takes native endian.
            let ip_ne = u32::from_be(ip_be);
            return Ok(Ipv4Addr::from_bits(ip_ne).to_string());
        }
    }
    Err(anyhow!("Cannot find default gateway"))
}

/// This test runs on the host Linux.
#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::ensure;
    use std::process::Command;

    fn is_tool_available(tool_name: &str) -> bool {
        Command::new("which").arg(tool_name).output().is_ok_and(|which| which.status.success())
    }

    #[test]
    fn test_get_default_gateway() -> Result<(), Error> {
        let gateway = get_default_gateway()?;

        let ip_route = if is_tool_available("route") {
            Command::new("bash")
                .arg("-c")
                .arg("route | grep -e '^default' | awk '{print $2}'")
                .output()?
        } else if is_tool_available("ip") {
            Command::new("bash")
                .arg("-c")
                .arg("ip route | grep -e '^default' | awk '{print $3}'")
                .output()?
        } else {
            println!("Skipping test since no suitable tool available");
            return Ok(());
        };
        let ip_route_gateway = str::from_utf8(&ip_route.stdout)
            .with_context(|| "Failed to run 'ip route'. {ip_route:?}")?
            .trim();
        ensure!(!ip_route_gateway.is_empty(), "Failed to find default route. {ip_route:?}");

        assert_eq!(gateway, ip_route_gateway);
        Ok(())
    }
}
