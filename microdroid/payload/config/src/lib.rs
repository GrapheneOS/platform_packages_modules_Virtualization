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

use serde::{Deserialize, Serialize};

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
