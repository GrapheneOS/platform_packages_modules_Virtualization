// Copyright 2025, The ChromiumOS Authors
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
//! Utility to format DropBox entries for AVF
use anyhow::Result;
use log::error;
use rustutils::system_properties;
use std::fmt::Write;
use std::fs;

const UNKNOWN_VALUE: &str = "UNKNOWN";
const SUBSYSTEM: &str = "AVF";

/// Adds header metadata about the device to the supplied message text,
/// returning a string intended for use as a DropBox entry.
pub fn build_dropbox_report(vm_info: &str, text: &str) -> Result<String> {
    let mut report = build_report_header()?;
    writeln!(&mut report, "VM Info: {}", vm_info)?;
    writeln!(&mut report, "Message: {}", text)?;
    Ok(report)
}

fn build_report_header() -> Result<String> {
    let mut header = String::new();
    writeln!(&mut header, "Build: {}", &get_system_property("ro.build.fingerprint"))?;
    writeln!(&mut header, "Hardware: {}", &get_system_property("ro.product.device"))?;
    writeln!(&mut header, "Model: {}", &get_model())?;
    writeln!(&mut header, "Revision: {}", &get_system_property("ro.revision"))?;
    writeln!(&mut header, "Kernel: {}", &get_kernel_version())?;
    writeln!(&mut header, "Subsystem: {}", SUBSYSTEM)?;
    writeln!(header)?;
    Ok(header)
}

fn get_system_property(property_name: &str) -> String {
    match system_properties::read(property_name) {
        Ok(Some(value)) if value.is_empty() => {
            error!("Property '{property_name}' is empty");
            UNKNOWN_VALUE.to_string()
        }
        Ok(Some(value)) => value,
        Ok(None) => {
            error!("Property '{property_name}' is missing");
            UNKNOWN_VALUE.to_string()
        }
        Err(err) => {
            error!("Error reading property '{property_name}': {err}");
            UNKNOWN_VALUE.to_string()
        }
    }
}

fn get_kernel_version() -> String {
    const PROC_VERSION_PATH: &str = "/proc/version";
    let mut version = match fs::read_to_string(PROC_VERSION_PATH) {
        Ok(v) => v,
        Err(err) => {
            error!("Failed to read {PROC_VERSION_PATH}: {err}");
            return UNKNOWN_VALUE.to_string();
        }
    };
    version = version.trim().to_string();
    if version.is_empty() {
        error!("Kernel version is unexpectedly empty");
        return UNKNOWN_VALUE.to_string();
    }
    version
}

fn get_model() -> String {
    let hwid = get_system_property("ro.boot.product.hardware.id");
    extract_model_from_hwid(hwid)
}

fn extract_model_from_hwid(hwid: String) -> String {
    // e.g. ANAHERA-TY00 123-456-ABC-DEF -> ANAHERA-TY00
    let split_hwid_on_space: Vec<&str> = hwid.split(" ").collect();
    if split_hwid_on_space.len() < 2 {
        error!("Unable to determine model from HWID '{hwid}'");
        return UNKNOWN_VALUE.to_string();
    }
    // e.g. ANAHERA-TY00 -> anahera
    let split_hwid_on_dash = split_hwid_on_space[0].split("-").next();
    if let Some(split_hwid_on_dash) = split_hwid_on_dash {
        split_hwid_on_dash.to_string().to_lowercase()
    } else {
        split_hwid_on_space[0].to_lowercase()
    }
}
