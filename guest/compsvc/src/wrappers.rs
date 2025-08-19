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
        log::debug!("Prepare to connect to {AUTHFS_SERVICE_SOCKET_NAME}");
        RpcSession::new()
            .setup_unix_domain_client(AUTHFS_SERVICE_SOCKET_NAME)
            .with_context(|| format!("Failed to connect to {AUTHFS_SERVICE_SOCKET_NAME}"))
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
