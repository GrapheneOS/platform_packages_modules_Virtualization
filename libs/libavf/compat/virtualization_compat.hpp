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

#pragma once

#include <android/virtualization.h>

/**
 * A simple wrapper over android/virtualization.h LL-NDK APIs that provides support for doing an
 * API level check before calling an API that was introduced in new Android releases.
 */
extern "C" {

  /**
   * A wrapper over AVirtualMachine_startWithStopCallback.
   *
   * On devices running API level >= 37 this function will act as a passthrough to the
   * AVirtualMachine_startWithStopCallback and `supports_callback` will be set to true. On other
   * devices it will call AVirtualMachine_start and `supports_callback` will be set to false.
   */
  int AVirtualMachineCompat_startWithStopCallback(AVirtualMachine* _Nonnull vm,
                                                  const AVirtualMachine_stopCallback _Nullable callback,
                                                  void* _Null_unspecified data,
                                                  bool* _Nonnull supports_callback);
  /**
   * A simple wrapper over AVirtualMachine_addMemoryMapping.
   *
   * On devices running API level >=37 this function will act as a passthrough to the
   * AVirtualMachine_addMemoryMapping. On other devices it simply returns -ENOTSUP.
   */
  int AVirtualMachineCompat_addMemoryMapping(AVirtualMachine* _Nonnull vm, int fd,
                                             uint64_t rangeStart, uint64_t rangeEnd,
                                             uint64_t offset,
                                             enum AVirtualMachineMemoryMappingAttributes attrs);

  /**
   * A simple wrapper over AVirtualMachine_removeMemoryMapping.
   *
   * On devices running API level >= 37 this function will act as a passthrough to the
   * AVirtualMachine_removeMemoryMapping. On other devices it simply returns -ENOTSUP.
   */
  bool AVirtualMachineCompat_removeMemoryMapping(AVirtualMachine* _Nonnull vm, int memory_id);
} // extern "C"
