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

//! This is a `encryptedstore` helper library. It provides limited ability to lookup state of
//! encrypted storage.
use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;

/// An unformatted encryptedstore data disk contains a magic header!
pub const UNFORMATTED_STORAGE_MAGIC: &str = "UNFORMATTED-STORAGE";

/// Is the given disk unformatted? Storage setup requires zeroing the `UNFORMATTED_STORAGE_MAGIC` at
/// the beginning of disk!
pub fn needs_formatting(data_device: &Path) -> Result<bool> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(data_device)
        .with_context(|| format!("Failed to open {data_device:?}"))?;

    let mut buf = [0; UNFORMATTED_STORAGE_MAGIC.len()];
    file.read_exact(&mut buf)?;

    Ok(buf == UNFORMATTED_STORAGE_MAGIC.as_bytes())
}
