// Copyright 2024, The Android Open Source Project
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

#pragma once

// TODO(b/309090563): remove this file once build flags are exposed to aconfig.

namespace android {
namespace virtualization {

inline bool IsOpenDiceChangesFlagEnabled() {
#ifdef AVF_OPEN_DICE_CHANGES
    return AVF_OPEN_DICE_CHANGES;
#else
    return false;
#endif
}

inline bool IsVendorModulesFlagEnabled() {
#ifdef AVF_ENABLE_VENDOR_MODULES
    return AVF_ENABLE_VENDOR_MODULES;
#else
    return false;
#endif
}

inline bool IsEnableTpuAssignableDeviceFlagEnabled() {
#ifdef AVF_ENABLE_TPU_ASSIGNABLE_DEVICE
    return AVF_ENABLE_TPU_ASSIGNABLE_DEVICE;
#else
    return false;
#endif
}

} // namespace virtualization
} // namespace android
