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

//! Support for generating and signing an info file listing names and digests of generated
//! artifacts.

use crate::compos_key;
use crate::fsverity;
use anyhow::{anyhow, Context, Result};
use odsign_proto::odsign_info::OdsignInfo;
use protobuf::Message;
use std::fs::File;
use std::io::Write;
use std::os::unix::io::AsFd;
use std::path::Path;

const TARGET_DIRECTORY: &str = "/data/misc/apexdata/com.android.art/dalvik-cache";
const SIGNATURE_EXTENSION: &str = ".signature";

/// Accumulates and then signs information about generated artifacts.
pub struct ArtifactSigner<'a> {
    base_directory: &'a Path,
    file_digests: Vec<(String, String)>, // (File name, digest in hex)
}

impl<'a> ArtifactSigner<'a> {
    /// base_directory specifies the directory under which the artifacts are currently located;
    /// they will eventually be moved under TARGET_DIRECTORY once they are verified and activated.
    pub fn new(base_directory: &'a Path) -> Self {
        Self { base_directory, file_digests: Vec::new() }
    }

    pub fn add_artifact(&mut self, path: &Path) -> Result<()> {
        // The path we store is where the file will be when it is verified, not where it is now.
        let suffix = path
            .strip_prefix(self.base_directory)
            .context("Artifacts must be under base directory")?;
        let target_path = Path::new(TARGET_DIRECTORY).join(suffix);
        let target_path = target_path.to_str().ok_or_else(|| anyhow!("Invalid path"))?;

        let file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
        let digest = fsverity::measure(file.as_fd())?;
        let digest = hex::encode(digest);

        self.file_digests.push((target_path.to_owned(), digest));
        Ok(())
    }

    /// Consume this ArtifactSigner and write details of all its artifacts to the given path,
    /// with accompanying sigature file.
    pub fn write_info_and_signature(self, info_path: &Path) -> Result<()> {
        let mut info = OdsignInfo::new();
        info.file_hashes.extend(self.file_digests);
        let bytes = info.write_to_bytes()?;

        let signature = compos_key::sign(&bytes)?;

        let mut file =
            File::create(info_path).with_context(|| format!("Creating {}", info_path.display()))?;
        file.write_all(&bytes)?;

        let mut signature_name = info_path.file_name().unwrap().to_owned();
        signature_name.push(SIGNATURE_EXTENSION);
        let signature_path = info_path.with_file_name(&signature_name);
        let mut signature_file = File::create(&signature_path)
            .with_context(|| format!("Creating {}", signature_path.display()))?;
        signature_file.write_all(&signature)?;

        Ok(())
    }
}
