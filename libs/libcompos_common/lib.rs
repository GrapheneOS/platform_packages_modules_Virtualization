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

//! Common items used by CompOS server and/or clients

pub mod binder;
pub mod compos_client;
pub mod odrefresh;
pub mod timeouts;

/// VSock port that the CompOS server listens on for RPC binder connections. This should be out of
/// future port range (if happens) that microdroid may reserve for system components.
pub const COMPOS_VSOCK_PORT: u32 = 6432;

/// The root directory where the CompOS APEX is mounted (read only).
pub const COMPOS_APEX_ROOT: &str = "/apex/com.android.compos";

/// The root of the  data directory available for private use by the CompOS APEX.
pub const COMPOS_DATA_ROOT: &str = "/data/misc/apexdata/com.android.compos";

/// The sub-directory where we store information relating to the instance of CompOS used for
/// real compilation.
pub const CURRENT_INSTANCE_DIR: &str = "current";

/// The sub-directory where we store information relating to the instance of CompOS used for
/// tests.
pub const TEST_INSTANCE_DIR: &str = "test";

/// The file that holds the instance_id of CompOS instance.
pub const INSTANCE_ID_FILE: &str = "instance_id";

/// The file that holds the instance image for a CompOS instance.
pub const INSTANCE_IMAGE_FILE: &str = "instance.img";

/// The file that holds the idsig for the CompOS Payload APK.
pub const IDSIG_FILE: &str = "idsig";

/// The file that holds the idsig for the build manifest APK that makes enumerated files from
/// /system available in CompOS.
pub const IDSIG_MANIFEST_APK_FILE: &str = "idsig_manifest_apk";

/// The file that holds the idsig for the build manifest APK that makes enumerated files from
/// /system_ext available in CompOS.
pub const IDSIG_MANIFEST_EXT_APK_FILE: &str = "idsig_manifest_ext_apk";

/// The Android path of fs-verity build manifest APK for /system.
pub const BUILD_MANIFEST_APK_PATH: &str = "/system/etc/security/fsverity/BuildManifest.apk";

/// The Android path of fs-verity build manifest APK for /system_ext.
pub const BUILD_MANIFEST_SYSTEM_EXT_APK_PATH: &str =
    "/system_ext/etc/security/fsverity/BuildManifestSystemExt.apk";

/// Returns the path of proper VM config for the current device.
pub fn get_vm_config_path(has_system_ext: bool, prefer_staged: bool) -> String {
    match (has_system_ext, prefer_staged) {
        (false, false) => "assets/vm_config.json",
        (false, true) => "assets/vm_config_staged.json",
        (true, false) => "assets/vm_config_system_ext.json",
        (true, true) => "assets/vm_config_system_ext_staged.json",
    }
    .to_owned()
}
