/*
 * Copyright 2023 The Android Open Source Project
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

//! Handle parsing of APK manifest files.
//! The manifest file is written as XML text, but is stored in the APK
//! as Android binary compressed XML. This library is a wrapper around
//! a thin C++ wrapper around libandroidfw, which contains the same
//! parsing code as used by package manager and aapt2 (amongst other
//! things).

use anyhow::{bail, Context, Result};
use apkmanifest_bindgen::{
    extractManifestInfo, freeManifestInfo, getPackageName, getRollbackIndex, getVersionCode,
    hasRelaxedRollbackProtectionPermission,
};
use std::ffi::CStr;
use std::fs::File;
use std::path::Path;

/// Information extracted from the Android manifest inside an APK.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct ApkManifestInfo {
    /// The package name of the app.
    pub package: String,
    /// The version code of the app.
    pub version_code: u64,
    /// Rollback index of the app used in the sealing dice policy.
    /// This is only set if the apk manifest has USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION
    /// permission.
    pub rollback_index: Option<u32>,
    /// Whether manifest has USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION permission.
    pub has_relaxed_rollback_protection_permission: bool,
}

const ANDROID_MANIFEST: &str = "AndroidManifest.xml";

/// Find the manifest inside the given APK and return information from it.
pub fn get_manifest_info<P: AsRef<Path>>(apk_path: P) -> Result<ApkManifestInfo> {
    let apk = File::open(apk_path.as_ref())?;
    let manifest = apkzip::read_file(apk, ANDROID_MANIFEST)?;

    // Safety: The function only reads the memory range we specify and does not hold
    // any reference to it.
    let native_info = unsafe { extractManifestInfo(manifest.as_ptr() as _, manifest.len()) };
    if native_info.is_null() {
        bail!("Failed to parse manifest")
    };

    scopeguard::defer! {
        // Safety: The value we pass is the result of calling extractManifestInfo as required.
        // We must call this exactly once, after we have finished using it, which the scopeguard
        // ensures.
        unsafe { freeManifestInfo(native_info); }
    }

    // Safety: It is always safe to call this with a valid native_info, which we have,
    // and it always returns a valid nul-terminated C string with the same lifetime as native_info.
    // We immediately make a copy.
    let package = unsafe { CStr::from_ptr(getPackageName(native_info)) };
    let package = package.to_str().context("Invalid package name")?.to_string();

    // Safety: It is always safe to call this with a valid native_info, which we have.
    let version_code = unsafe { getVersionCode(native_info) };

    // Safety: It is always safe to call this with a valid native_info, which we have.
    let rollback_index = unsafe {
        let rollback_index = getRollbackIndex(native_info);
        if rollback_index.is_null() {
            None
        } else {
            Some(*rollback_index)
        }
    };

    // Safety: It is always safe to call this with a valid native_info, which we have.
    let has_relaxed_rollback_protection_permission =
        unsafe { hasRelaxedRollbackProtectionPermission(native_info) };

    Ok(ApkManifestInfo {
        package,
        version_code,
        rollback_index,
        has_relaxed_rollback_protection_permission,
    })
}
