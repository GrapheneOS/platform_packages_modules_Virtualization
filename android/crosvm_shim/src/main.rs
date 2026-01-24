// Copyright 2026, The Android Open Source Project
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

//! Crosvm shim to launch crosvm with different policy.

use anyhow::{Context, Result};
use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() -> Result<()> {
    let path = "/apex/com.android.virt/bin/crosvm";
    let args = std::env::args_os().skip(1);
    let error = Command::new(path).args(args).exec();
    Err(error).context("Failed to execute crosvm")
}
