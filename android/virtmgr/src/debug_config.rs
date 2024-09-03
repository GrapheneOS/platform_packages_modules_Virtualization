// Copyright 2023, The Android Open Source Project
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

//! Functions for AVF debug policy and debug level

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    VirtualMachineAppConfig::DebugLevel::DebugLevel, VirtualMachineConfig::VirtualMachineConfig,
};
use anyhow::{anyhow, Context, Error, Result};
use libfdt::{Fdt, FdtError};
use log::{info, warn};
use rustutils::system_properties;
use std::ffi::{CString, NulError};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use vmconfig::get_debug_level;

const CUSTOM_DEBUG_POLICY_OVERLAY_SYSPROP: &str =
    "hypervisor.virtualizationmanager.debug_policy.path";
const DEVICE_TREE_EMPTY_TREE_SIZE_BYTES: usize = 100; // rough estimation.

struct DPPath {
    node_path: CString,
    prop_name: CString,
}

impl DPPath {
    fn new(node_path: &str, prop_name: &str) -> Result<Self, NulError> {
        Ok(Self { node_path: CString::new(node_path)?, prop_name: CString::new(prop_name)? })
    }

    fn to_path(&self) -> PathBuf {
        // unwrap() is safe for to_str() because node_path and prop_name were &str.
        PathBuf::from(
            [
                "/proc/device-tree",
                self.node_path.to_str().unwrap(),
                "/",
                self.prop_name.to_str().unwrap(),
            ]
            .concat(),
        )
    }
}

static DP_LOG_PATH: LazyLock<DPPath> =
    LazyLock::new(|| DPPath::new("/avf/guest/common", "log").unwrap());
static DP_RAMDUMP_PATH: LazyLock<DPPath> =
    LazyLock::new(|| DPPath::new("/avf/guest/common", "ramdump").unwrap());
static DP_ADB_PATH: LazyLock<DPPath> =
    LazyLock::new(|| DPPath::new("/avf/guest/microdroid", "adb").unwrap());

/// Get debug policy value in bool. It's true iff the value is explicitly set to <1>.
fn get_debug_policy_bool(path: &Path) -> Result<bool> {
    let value = match fs::read(path) {
        Ok(value) => value,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => Err(error).with_context(|| format!("Failed to read {path:?}"))?,
    };

    // DT spec uses big endian although Android is always little endian.
    match u32::from_be_bytes(value.try_into().map_err(|_| anyhow!("Malformed value in {path:?}"))?)
    {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(anyhow!("Invalid value {value} in {path:?}")),
    }
}

/// Get property value in bool. It's true iff the value is explicitly set to <1>.
/// It takes path as &str instead of &Path, because we don't want OsStr.
fn get_fdt_prop_bool(fdt: &Fdt, path: &DPPath) -> Result<bool> {
    let (node_path, prop_name) = (&path.node_path, &path.prop_name);
    let node = match fdt.node(node_path) {
        Ok(Some(node)) => node,
        Err(error) if error != FdtError::NotFound => {
            Err(Error::msg(error)).with_context(|| format!("Failed to get node {node_path:?}"))?
        }
        _ => return Ok(false),
    };

    match node.getprop_u32(prop_name) {
        Ok(Some(0)) => Ok(false),
        Ok(Some(1)) => Ok(true),
        Ok(Some(_)) => Err(anyhow!("Invalid prop value {prop_name:?} in node {node_path:?}")),
        Err(error) if error != FdtError::NotFound => {
            Err(Error::msg(error)).with_context(|| format!("Failed to get prop {prop_name:?}"))
        }
        _ => Ok(false),
    }
}

/// Fdt with owned vector.
struct OwnedFdt {
    buffer: Vec<u8>,
}

impl OwnedFdt {
    fn from_overlay_onto_new_fdt(overlay_file_path: &Path) -> Result<Self> {
        let mut overlay_buf = match fs::read(overlay_file_path) {
            Ok(fdt) => fdt,
            Err(error) if error.kind() == ErrorKind::NotFound => Default::default(),
            Err(error) => {
                Err(error).with_context(|| format!("Failed to read {overlay_file_path:?}"))?
            }
        };

        let overlay_buf_size = overlay_buf.len();

        let fdt_estimated_size = overlay_buf_size + DEVICE_TREE_EMPTY_TREE_SIZE_BYTES;
        let mut fdt_buf = vec![0_u8; fdt_estimated_size];
        let fdt = Fdt::create_empty_tree(fdt_buf.as_mut_slice())
            .map_err(Error::msg)
            .context("Failed to create an empty device tree")?;

        if !overlay_buf.is_empty() {
            let overlay_fdt = Fdt::from_mut_slice(overlay_buf.as_mut_slice())
                .map_err(Error::msg)
                .with_context(|| "Malformed {overlay_file_path:?}")?;

            // SAFETY: Return immediately if error happens. Damaged fdt_buf and fdt are discarded.
            unsafe {
                fdt.apply_overlay(overlay_fdt).map_err(Error::msg).with_context(|| {
                    "Failed to overlay {overlay_file_path:?} onto empty device tree"
                })?;
            }
        }

        Ok(Self { buffer: fdt_buf })
    }

    fn as_fdt(&self) -> &Fdt {
        // SAFETY: Checked validity of buffer when instantiate.
        unsafe { Fdt::unchecked_from_slice(&self.buffer) }
    }
}

/// Debug configurations for debug policy.
#[derive(Debug, Default)]
pub struct DebugPolicy {
    log: bool,
    ramdump: bool,
    adb: bool,
}

impl DebugPolicy {
    /// Build from the passed DTBO path.
    pub fn from_overlay(path: &Path) -> Result<Self> {
        let owned_fdt = OwnedFdt::from_overlay_onto_new_fdt(path)?;
        let fdt = owned_fdt.as_fdt();

        Ok(Self {
            log: get_fdt_prop_bool(fdt, &DP_LOG_PATH)?,
            ramdump: get_fdt_prop_bool(fdt, &DP_RAMDUMP_PATH)?,
            adb: get_fdt_prop_bool(fdt, &DP_ADB_PATH)?,
        })
    }

    /// Build from the /avf/guest subtree of the host DT.
    pub fn from_host() -> Result<Self> {
        Ok(Self {
            log: get_debug_policy_bool(&DP_LOG_PATH.to_path())?,
            ramdump: get_debug_policy_bool(&DP_RAMDUMP_PATH.to_path())?,
            adb: get_debug_policy_bool(&DP_ADB_PATH.to_path())?,
        })
    }
}

/// Debug configurations for both debug level and debug policy
#[derive(Debug, Default)]
pub struct DebugConfig {
    pub debug_level: DebugLevel,
    debug_policy: DebugPolicy,
}

impl DebugConfig {
    pub fn new(config: &VirtualMachineConfig) -> Self {
        let debug_level = get_debug_level(config).unwrap_or(DebugLevel::NONE);
        let debug_policy = Self::get_debug_policy().unwrap_or_else(|| {
            info!("Debug policy is disabled");
            Default::default()
        });

        Self { debug_level, debug_policy }
    }

    fn get_debug_policy() -> Option<DebugPolicy> {
        let dp_sysprop = system_properties::read(CUSTOM_DEBUG_POLICY_OVERLAY_SYSPROP);
        let custom_dp = dp_sysprop.unwrap_or_else(|e| {
            warn!("Failed to read sysprop {CUSTOM_DEBUG_POLICY_OVERLAY_SYSPROP}: {e}");
            Default::default()
        });

        match custom_dp {
            Some(path) if !path.is_empty() => match DebugPolicy::from_overlay(Path::new(&path)) {
                Ok(dp) => {
                    info!("Loaded custom debug policy overlay {path}: {dp:?}");
                    Some(dp)
                }
                Err(err) => {
                    warn!("Failed to load custom debug policy overlay {path}: {err:?}");
                    None
                }
            },
            _ => match DebugPolicy::from_host() {
                Ok(dp) => {
                    info!("Loaded debug policy from host OS: {dp:?}");
                    Some(dp)
                }
                Err(err) => {
                    warn!("Failed to load debug policy from host OS: {err:?}");
                    None
                }
            },
        }
    }

    #[cfg(test)]
    /// Creates a new DebugConfig with debug level. Only use this for test purpose.
    pub(crate) fn new_with_debug_level(debug_level: DebugLevel) -> Self {
        Self { debug_level, ..Default::default() }
    }

    /// Get whether console output should be configred for VM to leave console and adb log.
    /// Caller should create pipe and prepare for receiving VM log with it.
    pub fn should_prepare_console_output(&self) -> bool {
        self.debug_level != DebugLevel::NONE || self.debug_policy.log || self.debug_policy.adb
    }

    /// Get whether debug apexes (MICRODROID_REQUIRED_APEXES_DEBUG) are required.
    pub fn should_include_debug_apexes(&self) -> bool {
        self.debug_level != DebugLevel::NONE || self.debug_policy.adb
    }

    /// Decision to support ramdump
    pub fn is_ramdump_needed(&self) -> bool {
        self.debug_level != DebugLevel::NONE || self.debug_policy.ramdump
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_avf_debug_policy_with_ramdump() -> Result<()> {
        let debug_policy =
            DebugPolicy::from_overlay("avf_debug_policy_with_ramdump.dtbo".as_ref()).unwrap();

        assert!(!debug_policy.log);
        assert!(debug_policy.ramdump);
        assert!(debug_policy.adb);

        Ok(())
    }

    #[test]
    fn test_read_avf_debug_policy_without_ramdump() -> Result<()> {
        let debug_policy =
            DebugPolicy::from_overlay("avf_debug_policy_without_ramdump.dtbo".as_ref()).unwrap();

        assert!(!debug_policy.log);
        assert!(!debug_policy.ramdump);
        assert!(debug_policy.adb);

        Ok(())
    }

    #[test]
    fn test_read_avf_debug_policy_with_adb() -> Result<()> {
        let debug_policy =
            DebugPolicy::from_overlay("avf_debug_policy_with_adb.dtbo".as_ref()).unwrap();

        assert!(!debug_policy.log);
        assert!(!debug_policy.ramdump);
        assert!(debug_policy.adb);

        Ok(())
    }

    #[test]
    fn test_read_avf_debug_policy_without_adb() -> Result<()> {
        let debug_policy =
            DebugPolicy::from_overlay("avf_debug_policy_without_adb.dtbo".as_ref()).unwrap();

        assert!(!debug_policy.log);
        assert!(!debug_policy.ramdump);
        assert!(!debug_policy.adb);

        Ok(())
    }

    #[test]
    fn test_invalid_sysprop_disables_debug_policy() -> Result<()> {
        let debug_policy =
            DebugPolicy::from_overlay("/a/does/not/exist/path.dtbo".as_ref()).unwrap();

        assert!(!debug_policy.log);
        assert!(!debug_policy.ramdump);
        assert!(!debug_policy.adb);

        Ok(())
    }

    #[test]
    fn test_new_with_debug_level() -> Result<()> {
        assert_eq!(
            DebugConfig::new_with_debug_level(DebugLevel::NONE).debug_level,
            DebugLevel::NONE
        );
        assert_eq!(
            DebugConfig::new_with_debug_level(DebugLevel::FULL).debug_level,
            DebugLevel::FULL
        );

        Ok(())
    }
}
