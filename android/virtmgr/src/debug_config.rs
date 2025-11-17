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

use crate::aidl;
use crate::dt_overlay;
use anyhow::{anyhow, Context, Error, Result};
use libfdt::{Fdt, FdtError};
use log::{info, warn};
use rustutils::android::system_properties;
use std::ffi::{CString, NulError};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use vmconfig::get_debug_level;

const CUSTOM_DEBUG_POLICY_OVERLAY_SYSPROP: &str =
    "hypervisor.virtualizationmanager.debug_policy.path";
const DUMP_DT_SYSPROP: &str = "hypervisor.virtualizationmanager.dump_device_tree";
const DEVICE_TREE_OVERLAY_SIZE_BYTES: usize = 400; // rough estimation.

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

    /// Returns path as &str instead of &Path, because we don't want OsStr.
    fn to_fdt_overlay_path(&self) -> CString {
        // Safe to expect() because both two shouldn't have NUL in the middle.
        // Compiler checks C String literal, and ctor of CString checks when it's instantiated.
        CString::new([c"/fragment/__overlay__".to_bytes(), self.node_path.to_bytes()].concat())
            .expect("Concatenating two strings without NUL in the middle")
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
fn get_fdt_prop_bool(fdt_overlay: &Fdt, path: &DPPath) -> Result<bool> {
    let (node_path, prop_name) = (&path.to_fdt_overlay_path(), &path.prop_name);
    let node = match fdt_overlay.node(node_path) {
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

/// Sets the DP value by creating its path as well.
fn set_fdt_prop(fdt: &mut Fdt, path: &DPPath, value: &[u8]) -> Result<()> {
    let mut node = fdt.find_or_add_node_mut(&path.to_fdt_overlay_path())?;
    node.setprop(&path.prop_name, value)?;
    Ok(())
}

/// Fdt with owned vector.
struct OwnedFdt {
    buffer: Vec<u8>,
}

impl OwnedFdt {
    fn try_load(path: &Path) -> Result<Self> {
        let buffer = fs::read(path).with_context(|| format!("Failed to read {path:?}"))?;

        // Check validity.
        let _ = Fdt::from_slice(&buffer)?;

        Ok(OwnedFdt { buffer })
    }

    fn as_fdt(&self) -> &Fdt {
        // SAFETY: Checked validity of buffer when instantiate.
        unsafe { Fdt::unchecked_from_slice(&self.buffer) }
    }
}

/// Debug configurations for debug policy.
#[derive(Default)]
pub struct DebugPolicy {
    log: bool,
    ramdump: bool,
    adb: bool,
    fdt: Option<OwnedFdt>,
}

impl fmt::Debug for DebugPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DebugPolicy")
            .field("log", &self.log)
            .field("ramdump", &self.ramdump)
            .field("adb", &self.adb)
            .finish()
    }
}

impl DebugPolicy {
    /// Build from the passed DTBO path.
    pub fn from_overlay(path: &Path) -> Result<Self> {
        let owned_fdt = OwnedFdt::try_load(path)?;
        let fdt = owned_fdt.as_fdt();

        Ok(Self {
            log: get_fdt_prop_bool(fdt, &DP_LOG_PATH)?,
            ramdump: get_fdt_prop_bool(fdt, &DP_RAMDUMP_PATH)?,
            adb: get_fdt_prop_bool(fdt, &DP_ADB_PATH)?,
            fdt: Some(owned_fdt),
        })
    }

    /// Build from the /avf/guest subtree of the host DT.
    pub fn from_host() -> Result<Self> {
        let log = get_debug_policy_bool(&DP_LOG_PATH.to_path())?;
        let ramdump = get_debug_policy_bool(&DP_RAMDUMP_PATH.to_path())?;
        let adb = get_debug_policy_bool(&DP_ADB_PATH.to_path())?;
        let fdt = if log || ramdump || adb {
            let mut buffer = vec![0_u8; DEVICE_TREE_OVERLAY_SIZE_BYTES];
            let fdt = dt_overlay::create_empty_device_tree_overlay(&mut buffer)?;
            set_fdt_prop(fdt, &DP_LOG_PATH, &[log as u8])?;
            set_fdt_prop(fdt, &DP_RAMDUMP_PATH, &[ramdump as u8])?;
            set_fdt_prop(fdt, &DP_ADB_PATH, &[adb as u8])?;
            Some(OwnedFdt { buffer })
        } else {
            None
        };
        Ok(Self { log, ramdump, adb, fdt })
    }
}

/// Debug configurations for both debug level and debug policy
#[derive(Debug, Default)]
pub struct DebugConfig {
    pub debug_level: aidl::DebugLevel,
    pub dump_device_tree: bool,
    debug_policy: DebugPolicy,
}

impl DebugConfig {
    pub fn new(config: &aidl::VirtualMachineConfig) -> Self {
        let debug_level = get_debug_level(config).unwrap_or(aidl::DebugLevel::NONE);
        let debug_policy = if matches!(config, aidl::VirtualMachineConfig::RawConfig(_)) {
            info!("Debug policy ignored for non-Microdroid VM");
            Default::default()
        } else {
            Self::try_load_debug_policy().unwrap_or_else(|_| {
                info!("Debug policy is ignored");
                Default::default()
            })
        };

        let dump_dt_sysprop = system_properties::read_bool(DUMP_DT_SYSPROP, false);
        let dump_device_tree = dump_dt_sysprop.unwrap_or_else(|e| {
            warn!("Failed to read sysprop {DUMP_DT_SYSPROP}: {e}");
            false
        });

        Self { debug_level, debug_policy, dump_device_tree }
    }

    pub fn get_debug_policy_overlay(&self) -> Option<&Fdt> {
        self.debug_policy.fdt.as_ref().map(|fdt| fdt.as_fdt())
    }

    fn try_load_debug_policy() -> Result<DebugPolicy> {
        let dp_sysprop = system_properties::read(CUSTOM_DEBUG_POLICY_OVERLAY_SYSPROP);
        let custom_dp = dp_sysprop.unwrap_or_default();

        match custom_dp {
            Some(path) if !path.is_empty() => {
                let dp = DebugPolicy::from_overlay(Path::new(&path));
                match dp {
                    Ok(ref dp) => info!("Loaded custom debug policy overlay {path}: {dp:?}"),
                    Err(ref err) => {
                        warn!("Failed to load custom debug policy overlay {path}: {err:?}")
                    }
                };
                dp
            }
            _ => {
                let dp = DebugPolicy::from_host();
                match dp {
                    Ok(ref dp) => info!("Loaded debug policy from host OS: {dp:?}"),
                    Err(ref err) => warn!("Failed to load debug policy from host OS: {err:?}"),
                };
                dp
            }
        }
    }

    #[cfg(test)]
    /// Creates a new DebugConfig with debug level. Only use this for test purpose.
    pub(crate) fn new_with_debug_level(debug_level: aidl::DebugLevel) -> Self {
        Self { debug_level, ..Default::default() }
    }

    /// Get whether console output should be configred for VM to leave console and adb log.
    /// Caller should create pipe and prepare for receiving VM log with it.
    pub fn should_prepare_console_output(&self) -> bool {
        self.debug_level != aidl::DebugLevel::NONE || self.debug_policy.log || self.debug_policy.adb
    }

    /// Get whether debug apexes (MICRODROID_REQUIRED_APEXES_DEBUG) are required.
    pub fn should_include_debug_apexes(&self) -> bool {
        self.debug_level != aidl::DebugLevel::NONE || self.debug_policy.adb
    }

    /// Decision to support ramdump
    pub fn is_ramdump_needed(&self) -> bool {
        self.debug_level != aidl::DebugLevel::NONE || self.debug_policy.ramdump
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
    fn test_invalid_sysprop_returns_error() -> Result<()> {
        let res = DebugPolicy::from_overlay("/a/does/not/exist/path.dtbo".as_ref());
        assert!(res.is_err());
        Ok(())
    }

    #[test]
    fn test_new_with_debug_level() -> Result<()> {
        assert_eq!(
            DebugConfig::new_with_debug_level(aidl::DebugLevel::NONE).debug_level,
            aidl::DebugLevel::NONE
        );
        assert_eq!(
            DebugConfig::new_with_debug_level(aidl::DebugLevel::FULL).debug_level,
            aidl::DebugLevel::FULL
        );

        Ok(())
    }
}
