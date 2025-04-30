// Copyright 2024 The Android Open Source Project
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

//! Device tree source (dts) for comparing device tree contents
//! i.e. sorted dts decompiled by `dtc -s -O dts`.

use anyhow::{anyhow, Result};
use libfdt::Fdt;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Device tree source (dts)
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Dts {
    dts: String,
}

impl Dts {
    /// Creates a device tree source from /proc/device-tree style directory
    pub fn from_fs(path: &Path) -> Result<Self> {
        let path = path.to_str().unwrap();
        let mut dtc = Self::get_dtc().unwrap();
        let cmd = dtc.args(["-f", "-s", "-I", "fs", "-O", "dts", path]);
        let res = cmd.output()?;
        if !res.status.success() {
            return Err(anyhow!("Failed to run {cmd:?}: res={res:?}"));
        }
        Ok(Self { dts: String::from_utf8(res.stdout)? })
    }

    /// Creates a device tree source from dtb
    pub fn from_dtb(path: &Path) -> Result<Self> {
        let path = path.to_str().unwrap();
        let mut dtc = Self::get_dtc().unwrap();
        let cmd = dtc.args(["-f", "-s", "-I", "dtb", "-O", "dts", path]);
        let res = cmd.output()?;
        if !res.status.success() {
            return Err(anyhow!("Failed to run {cmd:?}: res={res:?}"));
        }
        Ok(Self { dts: String::from_utf8(res.stdout)? })
    }

    /// Creates a device tree source from Fdt
    pub fn from_fdt(fdt: &Fdt) -> Result<Self> {
        let mut dtc = Self::get_dtc().unwrap();
        let cmd = dtc
            .args(["-f", "-s", "-I", "dtb", "-O", "dts"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());

        let mut child = cmd.spawn()?;
        {
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(fdt.as_slice())?;
            // Explicitly drop stdin to avoid indefinite blocking
        }

        let res = child.wait_with_output()?;
        if !res.status.success() {
            return Err(anyhow!("Failed to run {cmd:?}: res={res:?}"));
        }
        Ok(Self { dts: String::from_utf8(res.stdout)? })
    }

    fn get_dtc() -> Option<Command> {
        const DTC_STATIC: &str = "./dtc_static";

        if Path::new(DTC_STATIC).exists() {
            Some(Command::new(DTC_STATIC))
        } else {
            None
        }
    }
}
