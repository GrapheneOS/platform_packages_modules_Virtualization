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

#ifndef AUTHFS_FSVERITY_METADATA_H
#define AUTHFS_FSVERITY_METADATA_H

// This file contains the format of fs-verity metadata (.fsv_meta).
//
// The header format of .fsv_meta is:
//
// +-----------+---------------------------------------------+------------+
// |  Address  |  Description                                |    Size    |
// +-----------+---------------------------------------------+------------+
// |  0x0000   |  32-bit LE, version of the format           |     4      |
// |           |                                             |            |
// |  0x0004   |  fsverity_descriptor (see linux/fsverity.h) |    256     |
// |           |                                             |            |
// |  0x0104   |  32-bit LE, type of signature               |     4      |
// |           |  (0: NONE, 1: PKCS7, 2: RAW)                |            |
// |           |                                             |            |
// |  0x0108   |  32-bit LE, size of signature               |     4      |
// |           |                                             |            |
// |  0x010C   |  signature                                  | See 0x0108 |
// +-----------+---------------------------------------------+------------+
//
// After the header, merkle tree dump exists at the first 4K boundary. Usually it's 0x1000, but it
// could be, for example, 0x2000 or 0x3000, depending on the size of header.
//
// TODO(b/193113326): sync with build/make/tools/releasetools/fsverity_metadata_generator.py

#include <stddef.h>
#include <stdint.h>
#include <linux/fsverity.h>

const uint64_t CHUNK_SIZE = 4096;

// Give the macro value a name to export.
const uint8_t FSVERITY_HASH_ALG_SHA256 = FS_VERITY_HASH_ALG_SHA256;

enum class FSVERITY_SIGNATURE_TYPE : __le32 {
    NONE = 0,
    PKCS7 = 1,
    RAW = 2,
};

struct fsverity_metadata_header {
    __le32 version;
    fsverity_descriptor descriptor;
    FSVERITY_SIGNATURE_TYPE signature_type;
    __le32 signature_size;
} __attribute__((packed));

#endif   // AUTHFS_FSVERITY_METADATA_H
