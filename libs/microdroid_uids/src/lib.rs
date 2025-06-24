// Copyright 2023 The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! User and group IDs within Microdroid

/// Always the user ID of Root.
pub const ROOT_UID: u32 = 0;

// Android reserves UID/GIDs 6000-6499 for use by the system partition -
// see AID_SYSTEM_RESERVED_START.
// Within Microdroid we own the system partition, so they are free for our
// use. The Microdroid system image includes /system/ext/passwd and
// /system/ext/group files that allocate names to the IDs that we are
// using, so that tools like `ps` handle them correctly - see build targets
// microdroid_etc_passwd and microdroid_etc_group.
// (Our UIDs are entirely separate from Android's, but we use the same
// Bionic, and it uses the Android definitions - so using a reserved range
// helps avoid confusion.)

/// Group ID shared by all payload users.
pub const MICRODROID_PAYLOAD_GID: u32 = 6000;

/// User ID for the initial payload user.
pub const MICRODROID_PAYLOAD_UID: u32 = 6000;
