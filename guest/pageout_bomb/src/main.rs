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

//! pageout_bomb. madvise's all mapped userspace memory.

use anyhow::Context;
use std::os::fd::AsFd;
use std::os::fd::AsRawFd;
use std::os::fd::BorrowedFd;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;

const ADVICE: libc::c_int = libc::MADV_COLD;

fn main() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("pageout_bomb")
            .with_max_level(log::LevelFilter::Info),
    );
    if let Err(e) = try_main() {
        log::error!("pageout_bomb failed: {e:#}");
        std::process::exit(1);
    }
}

fn try_main() -> anyhow::Result<()> {
    // For each `/proc/$PID`.
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        // Ignore non-numeric entries, we only want to look at PID directories.
        let os_filename = entry.file_name();
        let Some(filename) = os_filename.to_str() else {
            continue;
        };
        let Ok(pid) = filename.parse() else {
            continue;
        };
        // TODO: Skip self pid and maybe kernel processes (they have empty maps files).
        if let Err(e) = pageout_process(pid) {
            log::error!("pageout failed for {}: {e:#}", entry.path().display());
        }
    }

    Ok(())
}

/// `man 2 pidfd_open`.
fn pidfd_open(pid: u32) -> std::io::Result<OwnedFd> {
    // Signature: int syscall(SYS_pidfd_open, pid_t pid, unsigned int flags)
    //
    // SAFETY: We pass the right type of arguments for the syscall. `SYS_pidfd_open` just creates
    // an FD, doesn't have any extra safety considerations.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` must be valid because the SYS_pidfd_open return was non-negative and `fd` is
    // owned because we just created it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

/// `man 2 process_madvise`.
fn process_madvise(
    pidfd: BorrowedFd,
    iovecs: &[libc::iovec],
    advice: libc::c_int,
) -> std::io::Result<usize> {
    // Signature: ssize_t process_madvise(int pidfd, const struct iovec iovec[.n],
    //                                    size_t n, int advice, unsigned int flags);
    //
    // SAFETY: We pass the right type of arguments for the syscall. `SYS_process_madvise` just
    // provides advice to the kernel, doesn't have any extra safety considerations.
    let n = unsafe {
        libc::syscall(
            libc::SYS_process_madvise,
            pidfd.as_raw_fd(),
            iovecs.as_ptr(),
            iovecs.len(),
            advice,
            0,
        )
    };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(n as usize)
}

/// Call `process_madvise` on all of the mapped ranges of `pid`.
fn pageout_process(pid: u32) -> anyhow::Result<()> {
    let pidfd = pidfd_open(pid).context("failed to pidfd_open")?;

    // Get list of mapped ranges in the target process.
    let mut ranges = proc_pid_maps(pid)?;
    let mapped_bytes = ranges.iter().map(|iovec| iovec.iov_len).sum::<usize>();

    // Call `process_madvise` on all the ranges.
    //
    // Some ranges are not `madvise`-able (like "[vdso]"), so we call it repeatedly, incrementing
    // the `iovec`s and dropping ranges hit `EINVAL` as needed.
    let mut advised_bytes = 0;
    while !ranges.is_empty() {
        let mut n = match process_madvise(pidfd.as_fd(), &ranges[..], ADVICE) {
            Ok(n) => n,
            Err(e) => {
                if e.raw_os_error() == Some(libc::EINVAL) {
                    log::debug!(
                        "{pid} process_madvise returned EINVAL; skipping range {:#0x?}",
                        (ranges[0].iov_base, ranges[0].iov_len),
                    );
                    ranges.remove(0);
                    continue;
                }
                return Err(e).context("failed to process_madvise");
            }
        };
        if n == 0 {
            break;
        }
        advised_bytes += n;
        while n > 0 && !ranges.is_empty() {
            let nn = std::cmp::min(n, ranges[0].iov_len);
            ranges[0].iov_base = ranges[0].iov_base.wrapping_byte_add(nn);
            ranges[0].iov_len -= nn;
            n -= nn;
            if ranges[0].iov_len == 0 {
                ranges.remove(0);
            }
        }
    }

    log::trace!("pid {pid}: madvise'd {} KiB of {}", advised_bytes / 1024, mapped_bytes / 1024);

    Ok(())
}

/// Read all the address ranges from `/proc/$PID/maps`. See `man 5 proc_pid_maps`.
fn proc_pid_maps(pid: u32) -> anyhow::Result<Vec<libc::iovec>> {
    let mut ranges = Vec::new();
    let contents = std::fs::read_to_string(format!("/proc/{pid}/maps"))
        .context("failed to read /proc/pid/maps")?;
    for line in contents.lines() {
        let Some((start, end)) = parse_proc_pid_maps_line(line) else {
            continue;
        };
        ranges.push(libc::iovec {
            iov_base: start as *mut _,
            iov_len: end.checked_sub(start).unwrap(),
        });
    }
    Ok(ranges)
}

/// Parses and returns the start and end address of a mapping from `/proc/$PID/maps`.
///
/// Returns `None` if the line can't be parsed.
fn parse_proc_pid_maps_line(line: &str) -> Option<(usize, usize)> {
    let (start, rest) = line.split_once('-')?;
    let start = usize::from_str_radix(start, 16).ok()?;
    let (end, _rest) = rest.split_once(' ')?;
    let end = usize::from_str_radix(end, 16).ok()?;
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_proc_pid_maps_line() {
        // Test some lines from `/proc/1/maps` in microdroid.
        let maps_string = "\
557c204000-557c24d000 r--p 00000000 fc:01 116                            /system/bin/init
557c250000-557c368000 r-xp 0004c000 fc:01 116                            /system/bin/init
557c368000-557c36d000 r--p 00164000 fc:01 116                            /system/bin/init
557c370000-557c371000 rw-p 0016c000 fc:01 116                            /system/bin/init
557c371000-557c372000 rw-p 00000000 00:00 0                              [anon:.bss]
7d8c803000-7d8c804000 ---p 00000000 00:00 0
7d8c804000-7d8c808000 rw-p 00000000 00:00 0
7d8c808000-7d8d804000 ---p 00000000 00:00 0
7d8d804000-7d8d808000 rw-p 00000000 00:00 0
7d8d808000-7d8e803000 ---p 00000000 00:00 0
";
        assert_eq!(
            maps_string.lines().map(parse_proc_pid_maps_line).collect::<Vec<_>>(),
            vec![
                Some((0x557c204000, 0x557c24d000)),
                Some((0x557c250000, 0x557c368000)),
                Some((0x557c368000, 0x557c36d000)),
                Some((0x557c370000, 0x557c371000)),
                Some((0x557c371000, 0x557c372000)),
                Some((0x7d8c803000, 0x7d8c804000)),
                Some((0x7d8c804000, 0x7d8c808000)),
                Some((0x7d8c808000, 0x7d8d804000)),
                Some((0x7d8d804000, 0x7d8d808000)),
                Some((0x7d8d808000, 0x7d8e803000)),
            ],
        );
    }
}
