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
#[cfg_attr(test, mockall::automock)]
pub mod paths {
    use std::path::PathBuf;

    /// Provides a mock point to allow rebasing paths on a fake root.
    /// In release builds this just transforms a &str into a PathBuf.
    pub fn root_rebase(input: &str) -> PathBuf {
        PathBuf::from(input)
    }
}
/// A wrapper for fsverity.
#[cfg_attr(test, mockall::automock)]
pub mod fsverity {
    use std::io;
    use std::os::unix::io::BorrowedFd;
    /// Wraps fsverity::enable.
    /// Needless lifetimes are to satisfy mockall.
    #[allow(clippy::needless_lifetimes)]
    pub fn enable<'a>(fd: BorrowedFd<'a>) -> io::Result<()> {
        fsverity::enable(fd)
    }
}
/// A wrapper for rstutils::system_properties.
#[cfg_attr(test, mockall::automock)]
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
    #[cfg(test)]
    use mockall::automock;
    use std::os::unix::io::RawFd;
    use std::path::Path;

    /// Wraps minijail::Command
    pub struct Command {
        /// The real minijail::Command for use in release builds.
        pub real_command: Option<minijail::Command>,

        /// Used by unit tests to track Commands across different expectations.
        #[cfg(test)]
        pub tag: u32,
    }

    /// Wraps minijail::Command.
    pub struct CommandFactory;

    #[cfg_attr(test, automock, allow(dead_code))]
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

            #[cfg(test)]
            return Ok(Command { real_command, tag: Default::default() });

            #[cfg(not(test))]
            Ok(Command { real_command })
        }
    }

    /// Wraps minijail:Minijail.
    pub struct Minijail {
        real_mini_jail: minijail::Minijail,
    }
    #[cfg_attr(test, automock, allow(dead_code))]
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
