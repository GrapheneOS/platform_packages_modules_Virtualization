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

//! Routines for handling payload

use crate::instance::ApexData;
use crate::ioutil::wait_for_file;
use anyhow::{Context, Result};
use log::{info, warn};
use microdroid_metadata::{read_metadata, ApexPayload, Metadata};
use std::time::Duration;

const PAYLOAD_METADATA_PATH: &str = "/dev/block/by-name/payload-metadata";
const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Loads payload metadata from /dev/block/by-name/payload-metadata
pub fn load_metadata() -> Result<Metadata> {
    info!("loading payload metadata...");
    let file = wait_for_file(PAYLOAD_METADATA_PATH, WAIT_TIMEOUT)?;
    read_metadata(file)
}

/// Loads (name, public_key, root_digest) from payload APEXes
pub fn get_apex_data_from_payload(metadata: &Metadata) -> Result<Vec<ApexData>> {
    metadata
        .apexes
        .iter()
        .map(|apex| {
            let apex_path = format!("/dev/block/by-name/{}", apex.partition_name);
            let extracted =
                apexutil::verify(&apex_path).context(format!("Failed to parse {}", &apex_path))?;
            if let Some(manifest_name) = &extracted.name {
                if &apex.name != manifest_name {
                    warn!("Apex named {} is named {} in its manifest", apex.name, manifest_name);
                }
            };
            Ok(ApexData {
                name: apex.name.clone(),
                manifest_name: extracted.name,
                manifest_version: extracted.version,
                public_key: extracted.public_key,
                root_digest: extracted.root_digest,
                last_update_seconds: apex.last_update_seconds,
                is_factory: apex.is_factory,
            })
        })
        .collect()
}

/// Convert vector of ApexData into Metadata
pub fn to_metadata(apex_data: &[ApexData]) -> Metadata {
    Metadata {
        apexes: apex_data
            .iter()
            .map(|data| ApexPayload {
                name: data.name.clone(),
                public_key: data.public_key.clone(),
                root_digest: data.root_digest.clone(),
                manifest_name: data.manifest_name.clone().unwrap_or_default(),
                manifest_version: data.manifest_version.unwrap_or_default(),
                last_update_seconds: data.last_update_seconds,
                is_factory: data.is_factory,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}
