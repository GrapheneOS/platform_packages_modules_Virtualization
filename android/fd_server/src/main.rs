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

//! This program is a constrained file/FD server to serve file requests through a remote binder
//! service. The file server is not designed to serve arbitrary file paths in the filesystem. On
//! the contrary, the server should be configured to start with already opened FDs, and serve the
//! client's request against the FDs
//!
//! For example, `exec 9</path/to/file fd_server --ro-fds 9` starts the binder service. A client
//! client can then request the content of file 9 by offset and size.

mod aidl;

use anyhow::{bail, Result};
use clap::Parser;
use log::debug;
use nix::sys::stat::{umask, Mode};
use rpcbinder::RpcServer;
use rustutils::inherited_fd::take_fd_ownership;
use std::collections::BTreeMap;
use std::fs::File;
use std::os::unix::io::OwnedFd;

use aidl::{FdConfig, FdService};
use authfs_fsverity_metadata::parse_fsverity_metadata;

// TODO(b/259920193): support dynamic port for multiple fd_server instances
const RPC_SERVICE_PORT: u32 = 3264;

fn parse_arg_ro_fds(arg: &str) -> Result<(i32, FdConfig)> {
    let result: Result<Vec<i32>, _> = arg.split(':').map(|x| x.parse::<i32>()).collect();
    let fds = result?;
    if fds.len() > 2 {
        bail!("Too many options: {}", arg);
    }
    Ok((
        fds[0],
        FdConfig::Readonly {
            file: take_fd_ownership(fds[0])?.into(),
            // Alternative metadata source, if provided
            alt_metadata: fds
                .get(1)
                .map(|fd| take_fd_ownership(*fd))
                .transpose()?
                .and_then(|f| parse_fsverity_metadata(f.into()).ok()),
        },
    ))
}

#[derive(Parser)]
struct Args {
    /// Read-only FD of file, with optional FD of corresponding .fsv_meta, joined with a ':'.
    /// Example: "1:2", "3".
    #[clap(long)]
    ro_fds: Vec<String>,

    /// Read-writable FD of file
    #[clap(long)]
    rw_fds: Vec<i32>,

    /// Read-only FD of directory
    #[clap(long)]
    ro_dirs: Vec<i32>,

    /// Read-writable FD of directory
    #[clap(long)]
    rw_dirs: Vec<i32>,

    /// A pipe FD for signaling the other end once ready
    #[clap(long)]
    ready_fd: Option<i32>,
}

/// Convert argument strings and integers to a form that is easier to use and handles ownership.
fn convert_args(args: Args) -> Result<(BTreeMap<i32, FdConfig>, Option<OwnedFd>)> {
    let mut fd_pool = BTreeMap::new();
    for arg in args.ro_fds {
        let (fd, config) = parse_arg_ro_fds(&arg)?;
        fd_pool.insert(fd, config);
    }
    for fd in args.rw_fds {
        let file: File = take_fd_ownership(fd)?.into();
        if file.metadata()?.len() > 0 {
            bail!("File is expected to be empty");
        }
        fd_pool.insert(fd, FdConfig::ReadWrite(file));
    }
    for fd in args.ro_dirs {
        fd_pool.insert(fd, FdConfig::InputDir(take_fd_ownership(fd)?));
    }
    for fd in args.rw_dirs {
        fd_pool.insert(fd, FdConfig::OutputDir(take_fd_ownership(fd)?));
    }
    let ready_fd = args.ready_fd.map(take_fd_ownership).transpose()?;
    Ok((fd_pool, ready_fd))
}

fn main() -> Result<()> {
    // SAFETY: This is very early in the process. Nobody has taken ownership of the inherited FDs
    // yet.
    unsafe { rustutils::inherited_fd::init_once()? };

    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("fd_server")
            .with_max_level(log::LevelFilter::Debug),
    );

    let args = Args::parse();
    let (fd_pool, mut ready_fd) = convert_args(args)?;

    // Allow open/create/mkdir from authfs to create with expecting mode. It's possible to still
    // use a custom mask on creation, then report the actual file mode back to authfs. But there
    // is no demand now.
    let old_umask = umask(Mode::empty());
    debug!("Setting umask to 0 (old: {:03o})", old_umask.bits());

    debug!("fd_server is starting as a rpc service.");
    let service = FdService::new_binder(fd_pool).as_binder();
    // TODO(b/259920193): Only accept connections from the intended guest VM.
    let server = RpcServer::new_vsock(service, libc::VMADDR_CID_ANY, RPC_SERVICE_PORT)?;
    debug!("fd_server is ready");

    // Close the ready-fd if we were given one to signal our readiness.
    drop(ready_fd.take());

    server.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_args() {
        // Check that the command parsing has been configured in a valid way.
        Args::command().debug_assert();
    }
}
