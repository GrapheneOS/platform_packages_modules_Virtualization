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

/// fs-verity version that we are using
pub const FS_VERITY_VERSION: u8 = 1;

/// Hash algorithm to use from linux/fsverity.h
pub const FS_VERITY_HASH_ALG_SHA256: u8 = 1;

/// Log 2 of the block size (only 4096 is supported now)
pub const FS_VERITY_LOG_BLOCKSIZE: u8 = 12;
