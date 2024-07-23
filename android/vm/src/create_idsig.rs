// Copyright 2022, The Android Open Source Project
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

//! Command to create or update an idsig for APK

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::IVirtualizationService::IVirtualizationService;
use binder::ParcelFileDescriptor;
use anyhow::{Context, Error};
use std::fs::{File, OpenOptions};
use std::path::Path;

/// Creates or update the idsig file by digesting the input APK file.
pub fn command_create_idsig(
    service: &dyn IVirtualizationService,
    apk: &Path,
    idsig: &Path,
) -> Result<(), Error> {
    let apk_file = File::open(apk).with_context(|| format!("Failed to open {:?}", apk))?;
    let idsig_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(idsig)
        .with_context(|| format!("Failed to create/open {:?}", idsig))?;
    service
        .createOrUpdateIdsigFile(
            &ParcelFileDescriptor::new(apk_file),
            &ParcelFileDescriptor::new(idsig_file),
        )
        .with_context(|| format!("Failed to create/update idsig for {:?}", apk))?;
    Ok(())
}
