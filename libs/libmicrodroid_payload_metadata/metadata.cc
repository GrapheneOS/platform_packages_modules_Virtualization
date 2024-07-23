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

#include "microdroid/metadata.h"

#include <android-base/endian.h>
#include <android-base/file.h>

using android::base::ErrnoError;
using android::base::Error;
using android::base::Result;

namespace android {
namespace microdroid {

Result<Metadata> ReadMetadata(const std::string& path) {
    std::string content;
    if (!base::ReadFileToString(path, &content)) {
        return ErrnoError() << "Failed to read " << path;
    }

    // read length prefix (4-byte, big-endian)
    uint32_t size;
    const size_t length_prefix_bytes = sizeof(size);
    if (content.size() < length_prefix_bytes) {
        return Error() << "Invalid metadata: size == " << content.size();
    }
    size = be32toh(*reinterpret_cast<uint32_t*>(content.data()));
    if (content.size() < length_prefix_bytes + size) {
        return Error() << "Invalid metadata: size(" << size << ") doesn't match content size("
                       << content.size() - length_prefix_bytes << ")";
    }
    content = content.substr(length_prefix_bytes, size);

    // parse content
    Metadata metadata;
    if (!metadata.ParseFromString(content)) {
        return Error() << "Can't parse Metadata from " << path;
    }
    return metadata;
}

Result<void> WriteMetadata(const Metadata& metadata, std::ostream& out) {
    // prepare content
    std::string content;
    if (!metadata.SerializeToString(&content)) {
        return Error() << "Failed to write protobuf.";
    }

    // write length prefix (4-byte, big-endian)
    uint32_t size = htobe32(static_cast<uint32_t>(content.size()));
    out.write(reinterpret_cast<const char*>(&size), sizeof(size));

    // write content
    out << content;

    return {};
}

} // namespace microdroid
} // namespace android