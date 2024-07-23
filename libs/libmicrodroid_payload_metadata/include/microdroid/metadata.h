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

#pragma once

#include <android-base/result.h>
#include <metadata.pb.h>

#include <iostream>
#include <string>

namespace android {
namespace microdroid {

base::Result<Metadata> ReadMetadata(const std::string& path);

base::Result<void> WriteMetadata(const Metadata& metadata, std::ostream& out);

} // namespace microdroid
} // namespace android
