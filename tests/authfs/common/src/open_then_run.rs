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

//! This is a test helper program that opens files and/or directories, then passes the file
//! descriptors to the specified command. When passing the file descriptors, they are mapped to the
//! specified numbers in the child process.

use anyhow::{bail, Context, Result};
use clap::{parser::ValuesRef, Arg, ArgAction};
use command_fds::{CommandFdExt, FdMapping};
use log::{debug, error};
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{OwnedFd, RawFd};
use std::process::Command;

// `PseudoRawFd` is just an integer and not necessarily backed by a real FD. It is used to denote
// the expecting FD number, when trying to set up FD mapping in the child process. The intention
// with this alias is to improve readability by distinguishing from actual RawFd.
type PseudoRawFd = RawFd;

struct OwnedFdMapping {
    owned_fd: OwnedFd,
    target_fd: PseudoRawFd,
}

impl OwnedFdMapping {
    fn into_fd_mapping(self) -> FdMapping {
        FdMapping { parent_fd: self.owned_fd, child_fd: self.target_fd }
    }
}

struct Args {
    ro_file_fds: Vec<OwnedFdMapping>,
    rw_file_fds: Vec<OwnedFdMapping>,
    dir_fds: Vec<OwnedFdMapping>,
    cmdline_args: Vec<String>,
}

fn parse_and_create_file_mapping<F>(
    values: Option<ValuesRef<'_, String>>,
    opener: F,
) -> Result<Vec<OwnedFdMapping>>
where
    F: Fn(&str) -> Result<OwnedFd>,
{
    if let Some(options) = values {
        options
            .map(|option| {
                // Example option: 10:/some/path
                let strs: Vec<&str> = option.split(':').collect();
                if strs.len() != 2 {
                    bail!("Invalid option: {}", option);
                }
                let fd = strs[0].parse::<PseudoRawFd>().context("Invalid FD format")?;
                let path = strs[1];
                Ok(OwnedFdMapping { target_fd: fd, owned_fd: opener(path)? })
            })
            .collect::<Result<_>>()
    } else {
        Ok(Vec::new())
    }
}

#[rustfmt::skip]
fn args_command() -> clap::Command {
    clap::Command::new("open_then_run")
        .arg(Arg::new("open-ro")
             .long("open-ro")
             .value_name("FD:PATH")
             .help("Open <PATH> read-only to pass as fd <FD>")
             .action(ArgAction::Append))
        .arg(Arg::new("open-rw")
             .long("open-rw")
             .value_name("FD:PATH")
             .help("Open/create <PATH> read-write to pass as fd <FD>")
             .action(ArgAction::Append))
        .arg(Arg::new("open-dir")
             .long("open-dir")
             .value_name("FD:DIR")
             .help("Open <DIR> to pass as fd <FD>")
             .action(ArgAction::Append))
        .arg(Arg::new("args")
             .help("Command line to execute with pre-opened FD inherited")
             .last(true)
             .required(true)
             .num_args(0..))
}

fn parse_args() -> Result<Args> {
    let matches = args_command().get_matches();

    let ro_file_fds = parse_and_create_file_mapping(matches.get_many("open-ro"), |path| {
        Ok(OwnedFd::from(
            OpenOptions::new()
                .read(true)
                .open(path)
                .with_context(|| format!("Open {} read-only", path))?,
        ))
    })?;

    let rw_file_fds = parse_and_create_file_mapping(matches.get_many("open-rw"), |path| {
        Ok(OwnedFd::from(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
                .with_context(|| format!("Open {} read-write", path))?,
        ))
    })?;

    let dir_fds = parse_and_create_file_mapping(matches.get_many("open-dir"), |path| {
        Ok(OwnedFd::from(
            OpenOptions::new()
                .custom_flags(libc::O_DIRECTORY)
                .read(true) // O_DIRECTORY can only be opened with read
                .open(path)
                .with_context(|| format!("Open {} directory", path))?,
        ))
    })?;

    let cmdline_args: Vec<_> =
        matches.get_many::<String>("args").unwrap().map(|s| s.to_string()).collect();

    Ok(Args { ro_file_fds, rw_file_fds, dir_fds, cmdline_args })
}

fn try_main() -> Result<()> {
    let args = parse_args()?;

    let mut command = Command::new(&args.cmdline_args[0]);
    command.args(&args.cmdline_args[1..]);

    // Set up FD mappings in the child process.
    let mut fd_mappings = Vec::new();
    fd_mappings.extend(args.ro_file_fds.into_iter().map(OwnedFdMapping::into_fd_mapping));
    fd_mappings.extend(args.rw_file_fds.into_iter().map(OwnedFdMapping::into_fd_mapping));
    fd_mappings.extend(args.dir_fds.into_iter().map(OwnedFdMapping::into_fd_mapping));
    command.fd_mappings(fd_mappings)?;

    debug!("Spawning {:?}", command);
    command.spawn()?;
    Ok(())
}

fn main() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("open_then_run")
            .with_max_level(log::LevelFilter::Debug),
    );

    if let Err(e) = try_main() {
        error!("Failed with {:?}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_command() {
        // Check that the command parsing has been configured in a valid way.
        args_command().debug_assert();
    }
}
