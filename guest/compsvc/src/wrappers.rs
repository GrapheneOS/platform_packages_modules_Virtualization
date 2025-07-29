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

//! wrapper is a module to enable unit testing by wrapping lower level functions in a mockable
//! module, struct or trait.
use anyhow::{Context, Result};
use authfs_aidl_interface::aidl::com::android::virt::fs::IAuthFsService::{
    IAuthFsService, AUTHFS_SERVICE_SOCKET_NAME,
};
use binder::Strong;
#[cfg(test)]
use mockall::automock;
use rpcbinder::RpcSession;

pub struct AuthFsFactory;

#[cfg_attr(test, automock, allow(dead_code))]
impl AuthFsFactory {
    pub fn new_authfs_service() -> Result<Strong<dyn IAuthFsService>> {
        log::debug!("Prepare to connect to {}", AUTHFS_SERVICE_SOCKET_NAME);
        RpcSession::new()
            .setup_unix_domain_client(AUTHFS_SERVICE_SOCKET_NAME)
            .with_context(|| format!("Failed to connect to {}", AUTHFS_SERVICE_SOCKET_NAME))
    }
}

pub mod minijail {
    #[cfg(test)]
    use mockall::automock;
    use std::os::unix::io::RawFd;
    use std::path::Path;

    pub struct Command {
        pub real_command: Option<minijail::Command>,
        #[cfg(test)]
        pub tag: u32, // Used by unit tests to track Commands across different expectations.
    }

    pub struct CommandFactory;

    #[cfg_attr(test, automock, allow(dead_code))]
    #[allow(clippy::ptr_arg)]
    impl CommandFactory {
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
    pub struct Minijail {
        real_mini_jail: minijail::Minijail,
    }
    #[cfg_attr(test, automock, allow(dead_code))]
    impl Minijail {
        pub fn new() -> minijail::Result<Self> {
            Ok(Self { real_mini_jail: minijail::Minijail::new()? })
        }
        pub fn run_command(&self, command: Command) -> minijail::Result<i32> {
            self.real_mini_jail.run_command(
                command.real_command.expect("wrapper::minijail::Command real_command is None"),
            )
        }
        pub fn wait(&self) -> minijail::Result<()> {
            self.real_mini_jail.wait()
        }
    }
}

// Mocking std::process::command is far too complex. Instead we wrap
// cmdline invocations in thin standalone functions.
#[cfg_attr(test, mockall::automock, allow(dead_code))]
pub mod command_line_helper {
    use anyhow::{bail, Context, Result};
    use std::ffi::OsString;
    use std::path::Path;
    use std::process::Command;
    pub fn run_derive_classpath(android_root: &Path) -> Result<String> {
        let classpaths_root = android_root.join("etc/classpaths");

        let mut bootclasspath_arg = OsString::new();
        bootclasspath_arg.push("--bootclasspath-fragment=");
        bootclasspath_arg.push(classpaths_root.join("bootclasspath.pb"));

        let mut systemserverclasspath_arg = OsString::new();
        systemserverclasspath_arg.push("--systemserverclasspath-fragment=");
        systemserverclasspath_arg.push(classpaths_root.join("systemserverclasspath.pb"));

        let result = Command::new("/apex/com.android.sdkext/bin/derive_classpath")
            .arg(bootclasspath_arg)
            .arg(systemserverclasspath_arg)
            .arg("/proc/self/fd/1")
            .output()
            .context("Failed to run derive_classpath")?;

        if !result.status.success() {
            bail!("derive_classpath returned {}", result.status);
        }
        String::from_utf8(result.stdout).context("Converting derive_classpath output")
    }
}

#[cfg_attr(test, automock, allow(dead_code))]
pub mod system_properties {
    pub fn write(name: &str, value: &str) -> rustutils::system_properties::error::Result<()> {
        rustutils::system_properties::write(name, value)
    }
    pub fn read_bool(
        name: &str,
        default_value: bool,
    ) -> rustutils::system_properties::error::Result<bool> {
        rustutils::system_properties::read_bool(name, default_value)
    }
}
