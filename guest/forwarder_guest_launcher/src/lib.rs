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

//! A library that handles port forwarding

use anyhow::{anyhow, Context, Result};
use log::debug;
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const NON_PREVILEGED_PORT_RANGE_START: u16 = 1024;
const TTYD_PORT: u16 = 7681;
const TCPSTATES_STATE_CLOSE: &str = "CLOSE";
const TCPSTATES_STATE_LISTEN: &str = "LISTEN";

/// Forward tcp port over vsock for host to connect to it.
pub fn forward_port(tcp_port: u16, vsock_port: u32) {
    let _ = Command::new("forwarder_guest")
        .arg("--local")
        .arg(format!("127.0.0.1:{}", tcp_port))
        .arg("--remote")
        .arg(format!("vsock:2:{}", vsock_port))
        .spawn();
}

#[derive(Debug)]
struct TcpState {
    lport: u16,
    rport: u16,
    comm: String,
    newstate: String,
}

fn is_forwardable_port(port: u16) -> bool {
    port >= NON_PREVILEGED_PORT_RANGE_START && port != TTYD_PORT
}

/// Monitor active ports, and notify map of port and comm when there's any change.
pub async fn monitor_active_ports<T>(mut listener: T) -> Result<()>
where
    T: AsyncFnMut(&HashMap<u16, String>) -> Result<()>,
{
    let mut cmd = Command::new("stdbuf")
        .arg("-oL")
        .arg("/usr/sbin/tcpstates-libbpf")
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = cmd.stdout.take().context("Failed to get stdout of tcpstates")?;
    let mut lines = BufReader::new(stdout).lines();
    let header_line = lines.next_line().await?.ok_or(anyhow!("Failed to get header line"))?;
    let header: Vec<_> = header_line.split_whitespace().collect();
    let lport = header
        .iter()
        .position(|col| *col == "LPORT")
        .ok_or(anyhow!("Failed to find LPORT from header"))?;
    let rport = header
        .iter()
        .position(|col| *col == "RPORT")
        .ok_or(anyhow!("Failed to find RPORT from header"))?;
    let comm = header
        .iter()
        .position(|col| *col == "COMM")
        .ok_or(anyhow!("Failed to find COMM from header"))?;
    let newstate = header
        .iter()
        .position(|col| *col == "NEWSTATE")
        .ok_or(anyhow!("Failed to find NEWSTATE from header"))?;

    debug!("Collecting already opened ports");
    let mut listening_ports: HashMap<_, _> = linux::net::get_listening_tcp4_ports_from_localhost()?
        .into_iter()
        .filter(|(x, _)| is_forwardable_port(*x))
        .inspect(|(port, comm)| debug!("Port {port} is already opened by {comm:?}"))
        .collect();

    listener(&listening_ports).await?;

    debug!("Starting monitoring ports");
    while let Some(line) = lines.next_line().await? {
        let items: Vec<_> = line.split_whitespace().collect();
        let state = TcpState {
            lport: items
                .get(lport)
                .ok_or(anyhow!("Failed to find LPORT"))?
                .parse()
                .context("Invalid LPORT format")?,
            rport: items
                .get(rport)
                .ok_or(anyhow!("Failed to find RPORT"))?
                .parse()
                .context("Invalid RPORT format")?,
            comm: items.get(comm).ok_or(anyhow!("Failed to find COMM"))?.to_string(),
            newstate: items.get(newstate).ok_or(anyhow!("Failed to find NEWSTATE"))?.to_string(),
        };
        if !is_forwardable_port(state.lport) {
            continue;
        }
        if state.rport > 0 {
            continue;
        }
        match state.newstate.as_str() {
            TCPSTATES_STATE_LISTEN => {
                debug!("New listening port {} by {}", state.lport, state.comm);
                listening_ports.insert(state.lport, state.comm);
            }
            TCPSTATES_STATE_CLOSE => {
                debug!("Listening port {} by {} is now closed", state.lport, state.comm);
                listening_ports.remove(&state.lport);
            }
            _ => continue,
        }
        listener(&listening_ports).await?;
    }

    Err(anyhow!("report_active_ports is terminated"))
}
