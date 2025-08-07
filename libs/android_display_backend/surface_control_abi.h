/*
 * Copyright 2025 The Android Open Source Project
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

#include <stdint.h>

extern "C" {

typedef struct ASurfaceControl ASurfaceControl;
typedef struct ASurfaceTransaction ASurfaceTransaction;
typedef struct ANativeWindow ANativeWindow;
typedef struct AHardwareBuffer AHardwareBuffer;

using pASurfaceControl_createFromWindow = ASurfaceControl* (*)(ANativeWindow * window,
                                                               const char* debug_name);

using pASurfaceControl_release = void (*)(ASurfaceControl* surface_control);

using pASurfaceTransaction_create = ASurfaceTransaction* (*)(void);

using pASurfaceTransaction_setBuffer = void (*)(ASurfaceTransaction* transaction,
                                                ASurfaceControl* surface_control,
                                                AHardwareBuffer* buffer, int32_t acquire_fence_fd);

using pASurfaceTransaction_apply = void (*)(ASurfaceTransaction* transaction);

using pASurfaceTransaction_delete = void (*)(ASurfaceTransaction* transaction);

} // extern "C"