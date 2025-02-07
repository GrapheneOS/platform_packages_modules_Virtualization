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

#pragma once

#include <stddef.h>
#include <stdint.h>

// Opaque structure holding information extracted from an APK manifest.
struct ApkManifestInfo;

extern "C" {

// Parse a binary XML encoded APK manifest and extract relevant information.
// The caller must free the returned pointer using freeManifestInfo.  Returns
// null if any error occurs. Does not retain any pointer to the manifest
// provided.
const ApkManifestInfo* extractManifestInfo(const void* manifest, size_t size);

// Frees an ApkManifestInfo allocated by extractManifestInfo; this invalidates
// the pointer and it must not be used again.
void freeManifestInfo(const ApkManifestInfo* info);

// Given a valid ApkManifestInfo pointer, return the package name of the APK, as
// a nul-terminated UTF-8 string. The pointer remains valid until the
// ApkManifestInfo is freed.
const char* getPackageName(const ApkManifestInfo* info);

// Given a valid ApkManifestInfo pointer, return the version code of the APK.
uint64_t getVersionCode(const ApkManifestInfo* info);

// Given a valid ApkManifestInfo pointer, return the value of the rollback
// index. If rollback index is not defined in the AndroidManifest.xml returns
// nullptr.
const uint32_t* getRollbackIndex(const ApkManifestInfo* info);

// Give a valid ApkManifestInfo pointer, return the value of the
// has_relaxed_rollback_protection_permission.
bool hasRelaxedRollbackProtectionPermission(const ApkManifestInfo* info);
}
