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

//! Applies debug policies when booting microdroid

use rustutils::system_properties;
use rustutils::system_properties::error::PropertyWatcherError;
use std::fs::File;
use std::io::Read;

/// Get debug policy value in bool. It's true iff the value is explicitly set to <1>.
fn get_debug_policy_bool(path: &'static str) -> Option<bool> {
    let mut file = File::open(path).ok()?;
    let mut log: [u8; 4] = Default::default();
    file.read_exact(&mut log).ok()?;
    // DT spec uses big endian although Android is always little endian.
    Some(u32::from_be_bytes(log) == 1)
}

fn main() -> Result<(), PropertyWatcherError> {
    // If VM is debuggable or debug policy says so, send logs to outside ot the VM via the serial
    // console. Otherwise logs are internally consumed at /dev/null
    let log_path = if system_properties::read_bool("ro.boot.microdroid.debuggable", false)?
        || get_debug_policy_bool("/proc/device-tree/avf/guest/common/log").unwrap_or_default()
    {
        "/dev/hvc2"
    } else {
        "/dev/null"
    };
    system_properties::write("ro.log.file_logger.path", log_path)?;

    let (adbd_enabled, debuggable) = if system_properties::read_bool("ro.boot.adb.enabled", false)?
        || get_debug_policy_bool("/proc/device-tree/avf/guest/microdroid/adb").unwrap_or_default()
    {
        // debuggable is required for adb root and bypassing adb authorization.
        ("1", "1")
    } else {
        ("0", "0")
    };
    system_properties::write("init_debug_policy.adbd.enabled", adbd_enabled)?;
    system_properties::write("ro.debuggable", debuggable)?;

    Ok(())
}
