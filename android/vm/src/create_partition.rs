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

//! Command to create an empty partition

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::IVirtualizationService::IVirtualizationService;
use android_system_virtualizationservice::aidl::android::system::virtualizationservice::PartitionType::PartitionType;
use binder::ParcelFileDescriptor;
use anyhow::{Context, Error};
use std::convert::TryInto;
use std::fs::OpenOptions;
use std::path::Path;

/// Initialise an empty partition image of the given size to be used as a writable partition.
pub fn command_create_partition(
    service: &dyn IVirtualizationService,
    image_path: &Path,
    size: u64,
    partition_type: PartitionType,
) -> Result<(), Error> {
    let image = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(image_path)
        .with_context(|| format!("Failed to create {:?}", image_path))?;
    service
        .initializeWritablePartition(
            &ParcelFileDescriptor::new(image),
            size.try_into()?,
            partition_type,
        )
        .context(format!(
            "Failed to initialize partition type: {:?}, size: {}",
            partition_type, size
        ))?;
    Ok(())
}
