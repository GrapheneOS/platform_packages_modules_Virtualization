/*
 * Copyright (C) 2025 The Android Open Source Project
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

//! Wraps lower level APIs so they can be mocked for unit tests.

/// Path related wrappers.
#[cfg_attr(enable_mock, mockall::automock)]
pub mod paths {
    use std::path::PathBuf;

    /// Provides a mock point to allow rebasing paths on a fake root.
    /// In release builds this just transforms a &str into a PathBuf.
    pub fn root_rebase(input: &str) -> PathBuf {
        PathBuf::from(input)
    }
}
/// A wrapper for fsverity.
#[cfg_attr(enable_mock, mockall::automock)]
pub mod fsverity {
    use anyhow::{anyhow, Context, Result};
    use authfs_fsverity_metadata as fsvmeta;
    use std::convert::TryInto;
    use std::fs;
    use std::io;
    use std::os::unix::io::{AsRawFd, BorrowedFd};

    /// Wraps fsverity::enable.
    /// Needless lifetimes are to satisfy mockall.
    #[allow(clippy::needless_lifetimes)]
    pub fn enable<'a>(fd: BorrowedFd<'a>) -> io::Result<()> {
        fsverity::enable(fd)
    }

    /// Wraps fsverity::read_digest
    /// Needless lifetimes are to satisfy mockall.
    #[allow(clippy::needless_lifetimes)]
    pub fn read_digest<'a>(fd: BorrowedFd<'a>) -> io::Result<[u8; 32]> {
        fsverity::read_sha256_digest(fd)
    }

    /// If an fsv meta for the file exists implicitly trust it
    /// and return the fsverity sha256-digest from that file.
    #[allow(clippy::needless_lifetimes)]
    pub fn read_digest_from_fsv_meta<'a>(fd: BorrowedFd<'a>) -> Result<[u8; 32]> {
        let raw_fd = fd.as_raw_fd();
        let fd_path = format!("/proc/self/fd/{}", raw_fd);
        let fd_path = fs::read_link(&fd_path)
            .with_context(|| format!("Failed to read link for {}", fd_path))?;

        let fsv_meta = {
            let fsv_meta_path = fsvmeta::get_fsverity_metadata_path(&fd_path);
            let fsv_meta_file = fs::File::open(&fsv_meta_path)?;
            fsvmeta::parse_fsverity_metadata(fsv_meta_file)?
        };
        fsv_meta.digest.try_into().map_err(|_| anyhow!("Unexpected vector length"))
    }
}
/// A wrapper for rstutils::system_properties.
#[cfg_attr(enable_mock, mockall::automock)]
pub mod system_properties {
    /// Wraps rustutils::android::system_properties::write
    pub fn write(
        name: &str,
        value: &str,
    ) -> rustutils::android::system_properties::error::Result<()> {
        rustutils::android::system_properties::write(name, value)
    }
    /// Wraps rustutils::android::system_properties::read_bool
    pub fn read_bool(
        name: &str,
        default_value: bool,
    ) -> rustutils::android::system_properties::error::Result<bool> {
        rustutils::android::system_properties::read_bool(name, default_value)
    }
    /// Wraps rustutils::android::system_properties::read.
    pub fn read(
        name: &str,
    ) -> rustutils::android::system_properties::error::Result<Option<String>> {
        rustutils::android::system_properties::read(name)
    }
    /// Wraps rustutils::android::system_properties::foreach.
    pub fn foreach<'a, F>(f: F) -> rustutils::android::system_properties::error::Result<()>
    where
        F: FnMut(&str, &str) + 'a,
    {
        rustutils::android::system_properties::foreach(f)
    }
}

/// A wrapper for minijail.
pub mod minijail {
    use libc::pid_t;
    #[cfg(enable_mock)]
    use mockall::automock;
    use std::os::unix::io::RawFd;
    use std::path::Path;

    /// Wraps minijail::Command
    pub struct Command {
        /// The real minijail::Command for use in release builds.
        pub real_command: Option<minijail::Command>,

        /// Used by unit tests to track Commands across different expectations.
        #[cfg(enable_mock)]
        pub tag: u32,
    }

    /// Wraps minijail::Command.
    pub struct CommandFactory;

    #[cfg_attr(enable_mock, automock, allow(dead_code))]
    #[allow(clippy::ptr_arg)]
    impl CommandFactory {
        /// Wraps minijail::Command::new_for_path, generics were stripped
        /// to allow for mocking.
        pub fn new_for_path(
            path: &Path,
            keep_fds: &Vec<RawFd>,
            args: &Vec<String>,
            env_vars: &Vec<String>,
        ) -> minijail::Result<Command> {
            let real_command =
                Some(minijail::Command::new_for_path(path, keep_fds, args, Some(env_vars))?);

            #[cfg(enable_mock)]
            return Ok(Command { real_command, tag: Default::default() });

            #[cfg(not(enable_mock))]
            Ok(Command { real_command })
        }
    }

    /// Wraps minijail:Minijail.
    pub struct Minijail {
        real_mini_jail: minijail::Minijail,
    }
    #[cfg_attr(enable_mock, automock, allow(dead_code))]
    impl Minijail {
        /// Wraps minijail::Minijail::new.
        pub fn new() -> minijail::Result<Self> {
            Ok(Self { real_mini_jail: minijail::Minijail::new()? })
        }
        /// Wraps minijail::Minijail::run_command.
        pub fn run_command(&self, command: Command) -> minijail::Result<i32> {
            self.real_mini_jail.run_command(
                command.real_command.expect("wrapper::minijail::Command real_command is None"),
            )
        }

        /// Wraps minijail::Minijail::kill.
        pub fn kill(&self) -> minijail::Result<()> {
            self.real_mini_jail.kill()
        }

        /// Wraps minijail::Minijail::run except that it takes ownership of args
        /// in order to satisfy mockall.
        pub fn run<P: AsRef<Path> + 'static>(
            &self,
            cmd: P,
            inheritable_fds: &[RawFd],
            args: &[String],
        ) -> minijail::Result<pid_t> {
            self.real_mini_jail.run(cmd, inheritable_fds, args)
        }

        /// Wraps minijail::Minijail::wait.
        pub fn wait(&self) -> minijail::Result<()> {
            self.real_mini_jail.wait()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::unix::io::AsFd;

    #[test]
    fn test_fsverity_digest_match() {
        // The `data` property in Android.bp makes these files available
        // in the test's execution directory.
        let file = File::open("./testdata/input.4m").expect("Failed to open input.4m");
        let fd = file.as_fd();

        // 1. Enable fs-verity on the file. This should succeed.
        fsverity::enable(fd).expect("Failed to enable fs-verity");

        // 2. Read the digest directly from the kernel. This should also succeed.
        let kernel_digest = fsverity::read_digest(fd).expect("Failed to read digest from kernel");

        // 3. Read the digest from the .fsv_meta file. This should also succeed.
        let meta_digest = fsverity::read_digest_from_fsv_meta(fd)
            .expect("Failed to read digest from fsv_meta file");

        // 4. Verify that the digests match.
        assert_eq!(kernel_digest, meta_digest, "Kernel digest and fsv_meta digest do not match!");
    }
}
