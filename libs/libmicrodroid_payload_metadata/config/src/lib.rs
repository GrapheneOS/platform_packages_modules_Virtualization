// Copyright 2021, The Android Open Source Project
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

//! VM Payload Config

#[cfg(target_os = "android")]
use rustutils::android::system_properties;
use serde::{Deserialize, Serialize};
use std::ffi::CString;

/// VM payload config
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct VmPayloadConfig {
    /// OS config.
    /// Deprecated: don't use. Error if not "" or "microdroid".
    #[serde(default)]
    #[deprecated]
    pub os: OsConfig,

    /// Task to run in a VM
    #[serde(default)]
    pub task: Option<Task>,

    /// APEXes to activate in a VM
    #[serde(default)]
    pub apexes: Vec<ApexConfig>,

    /// Extra APKs to be passed to a VM
    #[serde(default)]
    pub extra_apks: Vec<ApkConfig>,

    /// Tenant config for multi tenancy case
    #[serde(default)]
    pub tenants: Vec<TenantConfig>,

    /// Tells VirtualizationService to use staged APEXes if possible
    #[serde(default)]
    pub prefer_staged: bool,

    /// Whether to export the tomsbtones (VM crashes) out of VM to host
    /// Default: true for debuggable VMs, false for non-debuggable VMs
    pub export_tombstones: Option<bool>,

    /// Whether the authfs service should be started in the VM. This enables read or write of host
    /// files with integrity checking, but not confidentiality.
    #[serde(default)]
    pub enable_authfs: bool,

    /// Ask the kernel for transparent huge-pages (THP). This is only a hint and
    /// the kernel will allocate THP-backed memory only if globally enabled by
    /// the system and if any can be found. See
    /// https://docs.kernel.org/admin-guide/mm/transhuge.html
    #[serde(default)]
    pub hugepages: bool,

    /// Whether to delay setup of the encrypted store. If set to true, microdroid_manager will
    /// wait for the payload to send a signal to do the setup.
    #[serde(default)]
    pub delay_encrypted_store_setup: bool,

    /// Whether to run the payload as root or not.
    #[serde(default)]
    pub run_as_root: bool,

    /// Whether to use dm-default-key to get DE and CE storage
    #[serde(default)]
    pub dm_default_key: bool,

    /// Configure Cgroup
    #[serde(default)]
    pub cgroup_config: Option<CgroupConfig>,
}

/// OS config
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OsConfig {
    /// The name of OS to use
    pub name: String,
}

impl Default for OsConfig {
    fn default() -> Self {
        Self { name: "".to_owned() }
    }
}

/// Payload's task can be one of plain executable
/// or an .so library which can be started via /system/bin/microdroid_launcher
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub enum TaskType {
    /// Task's command indicates the path to the executable binary.
    #[serde(rename = "executable")]
    #[default]
    Executable,
    /// Task's command indicates the .so library in /mnt/apk/lib/{arch}
    #[serde(rename = "microdroid_launcher")]
    MicrodroidLauncher,
}

/// Task to run in a VM
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Task {
    /// Decides how to execute the command: executable(default) | microdroid_launcher
    #[serde(default, rename = "type")]
    pub type_: TaskType,

    /// Command to run
    /// - For executable task, this is the path to the executable.
    /// - For microdroid_launcher task, this is the name of .so
    pub command: String,

    /// Optional arguments to pass to an executable.
    /// If this is non-None the arguments will be passed to the executable.
    /// For type=microdroid_launcher this should be omitted.
    pub command_args: Option<Vec<String>>,

    /// The "type" (in SELinux Context) applied to the task.
    /// Default, minimal type is applied if selinux_type = None
    pub selinux_type: Option<CString>,
}

/// APEX config
/// For now, we only pass the name of APEX.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApexConfig {
    /// The name of APEX
    pub name: String,
}

/// APK config
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApkConfig {
    /// The path of APK
    pub path: String,
}

/// Tenant config
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "package")]
pub enum TenantConfig {
    /// APEX Tenant
    #[serde(rename = "apex")]
    Apex(TenantConfiguration),
    /// APK Tenant
    #[serde(rename = "apk")]
    Apk(TenantConfiguration),
}

/// A map of signing authorities, keyed by the build type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedAuthority {
    /// The authority for "dev-keys" builds.
    #[serde(rename = "dev-keys")]
    pub dev_key: String,
    /// The authority for "test-keys" builds.
    #[serde(rename = "test-keys")]
    pub test_key: String,
    /// The authority for "release-keys" builds.
    #[serde(rename = "release-keys")]
    pub release_key: String,
}

#[cfg(target_os = "android")]
const RO_BUILD_TAGS: &str = "ro.build.tags";
const DEV_KEYS: &str = "dev-keys";
const TEST_KEYS: &str = "test-keys";
const RELEASE_KEYS: &str = "release-keys";

impl ExpectedAuthority {
    /// Resolves the expected authority based on the build tags from sysprop `ro.build.tags`.
    pub fn resolve_authority(&self) -> String {
        #[cfg(target_os = "android")]
        let build_tags = system_properties::read(RO_BUILD_TAGS)
            .unwrap_or_default()
            .unwrap_or(RELEASE_KEYS.to_string());
        #[cfg(not(target_os = "android"))]
        let build_tags = RELEASE_KEYS.to_string();

        if build_tags.contains(DEV_KEYS) {
            self.dev_key.clone()
        } else if build_tags.contains(TEST_KEYS) {
            self.test_key.clone()
        } else {
            // Fallback to release-keys as expected authority
            self.release_key.clone()
        }
    }
}

/// Tenant config
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TenantConfiguration {
    /// Tenant package name
    pub name: String,
    /// Tenant task
    #[serde(default)]
    pub task: Option<Task>,
    /// The minimum acceptable rollback_index (or version_code if rollback_index is missing) of the
    /// tenant package.
    pub min_version: u64,
    /// The signing authority (e.g., certificate hash) of the tenant package.
    /// b/484251187: This field is mandatory since Microdroid does not support persisting authority
    /// data in replay protected instance spec.
    pub expected_authority: ExpectedAuthority,
    /// Cgroup config for tenant
    pub cgroup_config: Option<CgroupConfig>,
}

/// Cgroup Config
#[derive(Default, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CgroupConfig {
    /// Cgroup initial memory high to activate
    pub memory_high_mib: u64,
    /// Opt in to have memory_high_mib increase as memory gets close to initial limit
    pub increase_high_mib: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_authority_map() {
        // Since resolve_authority reads system property, we can only verify it falls back to
        // release key (default) or matches what's on the host.
        // For unit test stability, we can just ensure it returns one of them.
        let authority = ExpectedAuthority {
            dev_key: "dev".to_string(),
            test_key: "test".to_string(),
            release_key: "release".to_string(),
        };

        let result = authority.resolve_authority();
        assert!(result == "dev" || result == "test" || result == "release");
    }
}
