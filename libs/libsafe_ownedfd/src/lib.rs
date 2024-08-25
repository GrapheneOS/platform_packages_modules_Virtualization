// Copyright 2024, The Android Open Source Project
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

//! Library for a safer conversion from `RawFd` to `OwnedFd`

use nix::fcntl::{fcntl, FdFlag, F_DUPFD, F_GETFD, F_SETFD};
use nix::libc;
use nix::unistd::close;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;
use std::os::fd::RawFd;
use std::sync::Mutex;
use thiserror::Error;

/// Errors that can occur while taking an ownership of `RawFd`
#[derive(Debug, PartialEq, Error)]
pub enum Error {
    /// RawFd is not a valid file descriptor
    #[error("{0} is not a file descriptor")]
    Invalid(RawFd),

    /// RawFd is either stdio, stdout, or stderr
    #[error("standard IO descriptors cannot be owned")]
    StdioNotAllowed,

    /// Generic UNIX error
    #[error("UNIX error")]
    Errno(#[from] nix::errno::Errno),
}

static LOCK: Mutex<()> = Mutex::new(());

/// Takes the ownership of `RawFd` and converts it to `OwnedFd`. It is important to know that
/// `RawFd` is closed when this function successfully returns. The raw file descriptor of the
/// returned `OwnedFd` is different from `RawFd`. The returned file descriptor is CLOEXEC set.
pub fn take_fd_ownership(raw_fd: RawFd) -> Result<OwnedFd, Error> {
    fcntl(raw_fd, F_GETFD).map_err(|_| Error::Invalid(raw_fd))?;

    if [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO].contains(&raw_fd) {
        return Err(Error::StdioNotAllowed);
    }

    // sync is needed otherwise we can create multiple OwnedFds out of the same RawFd
    let lock = LOCK.lock().unwrap();
    let new_fd = fcntl(raw_fd, F_DUPFD(raw_fd))?;
    close(raw_fd)?;
    drop(lock);

    // This is not essential, but let's follow the common practice in the Rust ecosystem
    fcntl(new_fd, F_SETFD(FdFlag::FD_CLOEXEC)).map_err(Error::Errno)?;

    // SAFETY: In this function, we have checked that RawFd is actually an open file descriptor and
    // this is the first time to claim its ownership because we just created it by duping.
    Ok(unsafe { OwnedFd::from_raw_fd(new_fd) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use nix::fcntl::{fcntl, FdFlag, F_GETFD, F_SETFD};
    use std::os::fd::AsRawFd;
    use std::os::fd::IntoRawFd;
    use tempfile::tempfile;

    #[test]
    fn good_fd() -> Result<()> {
        let raw_fd = tempfile()?.into_raw_fd();
        assert!(take_fd_ownership(raw_fd).is_ok());
        Ok(())
    }

    #[test]
    fn invalid_fd() -> Result<()> {
        let raw_fd = 12345; // randomly chosen
        assert_eq!(take_fd_ownership(raw_fd).unwrap_err(), Error::Invalid(raw_fd));
        Ok(())
    }

    #[test]
    fn original_fd_closed() -> Result<()> {
        let raw_fd = tempfile()?.into_raw_fd();
        let owned_fd = take_fd_ownership(raw_fd)?;
        assert_ne!(raw_fd, owned_fd.as_raw_fd());
        assert!(fcntl(raw_fd, F_GETFD).is_err());
        Ok(())
    }

    #[test]
    fn cannot_use_same_rawfd_multiple_times() -> Result<()> {
        let raw_fd = tempfile()?.into_raw_fd();

        let owned_fd = take_fd_ownership(raw_fd); // once
        let owned_fd2 = take_fd_ownership(raw_fd); // twice

        assert!(owned_fd.is_ok());
        assert!(owned_fd2.is_err());
        Ok(())
    }

    #[test]
    fn cloexec() -> Result<()> {
        let raw_fd = tempfile()?.into_raw_fd();

        // intentionally clear cloexec to see if it is set by take_fd_ownership
        fcntl(raw_fd, F_SETFD(FdFlag::empty()))?;
        let flags = fcntl(raw_fd, F_GETFD)?;
        assert_eq!(flags, FdFlag::empty().bits());

        let owned_fd = take_fd_ownership(raw_fd)?;
        let flags = fcntl(owned_fd.as_raw_fd(), F_GETFD)?;
        assert_eq!(flags, FdFlag::FD_CLOEXEC.bits());
        drop(owned_fd);
        Ok(())
    }
}
