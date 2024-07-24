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

//! Helpers for running odrefresh

use anyhow::{anyhow, Result};
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;

/// The path to the odrefresh binary
pub const ODREFRESH_PATH: &str = "/apex/com.android.art/bin/odrefresh";

/// The path under which odrefresh writes compiled artifacts
pub const ODREFRESH_OUTPUT_ROOT_DIR: &str = "/data/misc/apexdata/com.android.art";

/// The directory under ODREFRESH_OUTPUT_ROOT_DIR where pending artifacts are written
pub const PENDING_ARTIFACTS_SUBDIR: &str = "compos-pending";

/// The directory under ODREFRESH_OUTPUT_ROOT_DIR where test artifacts are written
pub const TEST_ARTIFACTS_SUBDIR: &str = "test-artifacts";

/// The directory under ODREFRESH_OUTPUT_ROOT_DIR where the current (active) artifacts are stored
pub const CURRENT_ARTIFACTS_SUBDIR: &str = "dalvik-cache";

/// Prefixes of system properties that are interested to odrefresh and dex2oat.
const ALLOWLIST_SYSTEM_PROPERTY_PREFIXES: &[&str] =
    &["dalvik.vm.", "ro.dalvik.vm.", "persist.device_config.runtime_native_boot."];

// The highest "standard" exit code defined in sysexits.h (as EX__MAX); odrefresh error codes
// start above here to avoid clashing.
// TODO: What if this changes?
const EX_MAX: i8 = 78;

/// The defined odrefresh exit codes - see art/odrefresh/include/odrefresh/odrefresh.h
#[derive(Debug, PartialEq, Eq, FromPrimitive)]
#[repr(i8)]
pub enum ExitCode {
    /// No compilation required, all artifacts look good
    Okay = 0,
    /// Compilation required
    CompilationRequired = EX_MAX + 1,
    /// New artifacts successfully generated
    CompilationSuccess = EX_MAX + 2,
    /// Compilation failed
    CompilationFailed = EX_MAX + 3,
    /// Removal of existing invalid artifacts failed
    CleanupFailed = EX_MAX + 4,
}

impl ExitCode {
    /// Map an integer to the corresponding ExitCode enum, if there is one
    pub fn from_i32(exit_code: i32) -> Result<Self> {
        FromPrimitive::from_i32(exit_code)
            .ok_or_else(|| anyhow!("Unexpected odrefresh exit code: {}", exit_code))
    }
}

/// Returns whether the system property name is interesting to odrefresh and dex2oat.
pub fn is_system_property_interesting(name: &str) -> bool {
    for prefix in ALLOWLIST_SYSTEM_PROPERTY_PREFIXES {
        if name.starts_with(prefix) {
            return true;
        }
    }
    false
}
