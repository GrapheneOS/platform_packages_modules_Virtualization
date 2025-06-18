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

//! Monitors cgroup of Microdroid

use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;

use inotify::Inotify;
use inotify::WatchMask;
use log::error;
use log::info;
use nix::errno::Errno;
use nix::sys::epoll::Epoll;
use nix::sys::epoll::EpollCreateFlags;
use nix::sys::epoll::EpollEvent;
use nix::sys::epoll::EpollFlags;
use nix::sys::epoll::EpollTimeout;
use nix::sys::eventfd::EventFd;
use std::fs::read_to_string;
use std::fs::write;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::ErrorKind;
use std::os::fd::AsFd;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const CGROUP_BASE_PATH: &str = "/sys/fs/cgroup/";
const HIGH_LIMIT_MULTIPLIER: i64 = 125;

// function to construct cgroup file paths
fn get_cgroup_file_path(cgroup_name: &str, filename: &str) -> PathBuf {
    PathBuf::from(format!("{}{}/{}", CGROUP_BASE_PATH, cgroup_name, filename))
}

// Function to read a i64 value from a cgroup file
fn read_cgroup_value(file_path: &Path) -> Result<i64> {
    let content = read_to_string(file_path)?;
    content.trim().parse::<i64>().context("Failed to parse value")
}

// Function to write an i64 value to a cgroup file
fn write_cgroup_value(file_path: &Path, value: i64) -> Result<()> {
    write(file_path, value.to_string()).context("Failed to write cgroup value")
}

fn get_high_breach_count(events_file_path: &Path) -> Result<i64> {
    let file = File::open(events_file_path)?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        if line.starts_with("high ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 2 {
                if let Ok(value) = parts[1].parse::<i64>() {
                    return Ok(value);
                }
            }
        }
    }
    anyhow::bail!("no 'high' found memory.events")
}

fn init_events_monitor(events_file_path: &Path) -> Result<Inotify> {
    let inotify = Inotify::init().context("failed to initialize inotify")?;

    inotify
        .watches()
        .add(events_file_path, WatchMask::MODIFY)
        .context("Failed to watch events file path")?;
    Ok(inotify)
}

fn handle_high_breach_event(
    current_usage_file_path: &Path,
    peak_usage_file_path: &Path,
    high_limit_file_path: &Path,
) -> Result<()> {
    info!(
        "cgroup {}: memory.current = {} bytes",
        current_usage_file_path.display(),
        read_cgroup_value(current_usage_file_path).unwrap_or(-1)
    );
    info!(
        "cgroup {}: memory.peak = {} bytes",
        peak_usage_file_path.display(),
        read_cgroup_value(peak_usage_file_path).unwrap_or(-1)
    );
    let current_high_limit_bytes = read_cgroup_value(high_limit_file_path)?;

    info!(
        "cgroup {}: Current memory.high limit: {} bytes",
        high_limit_file_path.display(),
        current_high_limit_bytes
    );

    let new_high_limit_bytes = current_high_limit_bytes
        .checked_mul(HIGH_LIMIT_MULTIPLIER)
        .context("overflow increasing high limit bytes")?
        / 100;
    info!("cgroup: Calculating new memory.high limit (x 1.25): {} bytes", new_high_limit_bytes);

    if let Err(e) = write_cgroup_value(high_limit_file_path, new_high_limit_bytes) {
        Err(anyhow!(
            "Failed to write new memory.high limit. Check permissions and value. Error: {:#}",
            e
        ))
    } else {
        info!("Successfully updated memory.high to {} bytes.", new_high_limit_bytes);
        Ok(())
    }
}

fn monitor_events(
    events_file_path: &Path,
    high_limit_file_path: &Path,
    current_usage_file_path: &Path,
    peak_usage_file_path: &Path,
    kill_switch: &Arc<EventFd>,
) -> Result<()> {
    let mut inotify =
        init_events_monitor(events_file_path).context("failed to spawn inotify events monitor")?;

    let mut old_high_event_count: i64 = 0;
    let mut buf = vec![0u8; 1024];

    let epoll = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)?;
    epoll
        .add(kill_switch.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 0))
        .context("failed to register kill switch")?;
    epoll
        .add(inotify.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 1))
        .context("failed to register inotify fd")?;
    let mut epoll_evts = [EpollEvent::empty()];
    loop {
        let epoll_res = epoll.wait(&mut epoll_evts, EpollTimeout::NONE);
        if let Err(e) = epoll_res {
            if e == Errno::EINTR {
                // Ignore interrupts and wait again
                continue;
            } else {
                return Err(e.into());
            }
        }
        match epoll_evts[0].data() {
            0 => {
                // Kill switch - exit thread
                return Ok(());
            }
            1 => {
                let events = inotify.read_events(&mut buf);
                if let Err(e) = events {
                    // if EINTR, retry
                    if e.kind() == ErrorKind::Interrupted
                        || e.kind() == ErrorKind::WouldBlock
                        || e.kind() == ErrorKind::UnexpectedEof
                    {
                        continue;
                    } else {
                        return Err(anyhow!("Error while reading memory events: {:#}", e));
                    }
                }
                let high_event_count = get_high_breach_count(events_file_path)?;

                if high_event_count == old_high_event_count {
                    continue;
                }

                error!("memory.high breach event detected");
                old_high_event_count = high_event_count;
                // If a high breach event is detected, we will increase the limit by 25%
                if let Err(e) = handle_high_breach_event(
                    current_usage_file_path,
                    peak_usage_file_path,
                    high_limit_file_path,
                ) {
                    error!("Error handling high breach event: {}", e);
                };
            }
            _ => {
                return Err(anyhow!("Unknown event received: {:?}", epoll_evts[0]));
            }
        }
    }
}

pub fn start_cgroup_monitor(
    cgroup_name: &'static str,
) -> Result<(thread::JoinHandle<()>, Arc<EventFd>)> {
    let cgroup_evt_fd = Arc::new(EventFd::new()?);
    let cgroup_evt_fd_clone = cgroup_evt_fd.clone();
    let cgroup_thread = thread::Builder::new()
        .name("microdroid_cgroup_monitor".to_string())
        .spawn(move || {
            let events_file_path = get_cgroup_file_path(cgroup_name, "memory.events");
            let high_limit_file_path = get_cgroup_file_path(cgroup_name, "memory.high");
            let current_usage_file_path = get_cgroup_file_path(cgroup_name, "memory.current");
            let peak_usage_file_path = get_cgroup_file_path(cgroup_name, "memory.peak");

            info!("Monitoring cgroup memory.events at {}", events_file_path.display());

            let high_val = read_cgroup_value(&high_limit_file_path);
            match high_val {
                Ok(val) => info!("cgroup: Current memory.high at {}", val),
                Err(e) => {
                    error!("Failed to read high value: {e}");
                    return;
                }
            }

            while let Err(e) = monitor_events(
                &events_file_path,
                &high_limit_file_path,
                &current_usage_file_path,
                &peak_usage_file_path,
                &cgroup_evt_fd_clone,
            ) {
                error!("cgroup monitor failed: {:#}", e);
                thread::sleep(Duration::from_secs(1));
            }
        })
        .context("failed to spawn cgroup monitor thread")?;
    Ok((cgroup_thread, cgroup_evt_fd))
}
