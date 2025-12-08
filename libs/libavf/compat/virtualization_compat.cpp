/*
 * Copyright (C) 2025 The Android Open Source Project
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

#define LOG_TAG "AvfCompatLib"

#include "virtualization_compat.hpp"

#include <android/api-level.h>
#include <errno.h>
#include <log/log.h>
#include <sys/cdefs.h>

extern "C" {

int AVirtualMachineCompat_startWithStopCallback(
        AVirtualMachine* _Nonnull vm, const AVirtualMachine_stopCallback _Nullable callback,
        void* _Null_unspecified data, bool* supports_callback) {
#if defined(__ANDROID_UNAVAILABLE_SYMBOLS_ARE_WEAK__) || __ANDROID_API__ >= 37
    if (__builtin_available(android 37, *)) {
        ALOGI("%s: passthrough to AVirtualMachine_startWithStopCallback", __func__);
        *supports_callback = true;
        return AVirtualMachine_startWithStopCallback(vm, callback, data);
    }
#else
    (void)callback;
    (void)data;
#endif // defined(__ANDROID_UNAVAILABLE_SYMBOLS_ARE_WEAK__) || __ANDROID_API__ >= 37
    ALOGI("%s: fallback to AVirtualMachine_start", __func__);
    *supports_callback = false;
    return AVirtualMachine_start(vm);
}

int AVirtualMachineCompat_addMemoryMapping(AVirtualMachine* _Nonnull vm, int fd,
                                           uint64_t rangeStart, uint64_t rangeEnd, uint64_t offset,
                                           enum AVirtualMachineMemoryMappingAttributes attrs) {
#if defined(__ANDROID_UNAVAILABLE_SYMBOLS_ARE_WEAK__) || __ANDROID_API__ >= 37
    if (__builtin_available(android 37, *)) {
        return AVirtualMachine_addMemoryMapping(vm, fd, rangeStart, rangeEnd, offset, attrs);
    }
#else
    (void)vm;
    (void)fd;
    (void)rangeStart;
    (void)rangeEnd;
    (void)offset;
    (void)attrs;
#endif // defined(__ANDROID_UNAVAILABLE_SYMBOLS_ARE_WEAK__)
    return -ENOTSUP;
}

bool AVirtualMachineCompat_removeMemoryMapping(AVirtualMachine* _Nonnull vm, int memory_id) {
#if defined(__ANDROID_UNAVAILABLE_SYMBOLS_ARE_WEAK__) || __ANDROID_API__ >= 37
    if (__builtin_available(android 37, *)) {
        return AVirtualMachine_removeMemoryMapping(vm, memory_id);
    }
#else
    (void)vm;
    (void)memory_id;
#endif // defined(__ANDROID_UNAVAILABLE_SYMBOLS_ARE_WEAK__)
    return false;
}

} // extern "C"
