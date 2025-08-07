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

#include <android-base/no_destructor.h>

#include <new>

#include "surface_control_abi.h"

class SurfaceControl {
public:
    static SurfaceControl& GetInstance();

    bool IsSupported() const;

    ASurfaceControl* ASurfaceControl_createFromWindow(ANativeWindow* window,
                                                      const char* debug_name);
    void ASurfaceControl_release(ASurfaceControl* surface_control);
    ASurfaceTransaction* ASurfaceTransaction_create();
    void ASurfaceTransaction_setBuffer(ASurfaceTransaction* transaction,
                                       ASurfaceControl* surface_control, AHardwareBuffer* buffer,
                                       int32_t acquire_fence_fd);
    void ASurfaceTransaction_apply(ASurfaceTransaction* transaction);
    void ASurfaceTransaction_delete(ASurfaceTransaction* transaction);

private:
    friend class android::base::NoDestructor<SurfaceControl>;

    SurfaceControl();
    ~SurfaceControl() = delete;

    bool LoadFunctions();

    bool is_supported_ = false;

    pASurfaceControl_createFromWindow ASurfaceControl_createFromWindow_ = nullptr;
    pASurfaceControl_release ASurfaceControl_release_ = nullptr;
    pASurfaceTransaction_create ASurfaceTransaction_create_ = nullptr;
    pASurfaceTransaction_setBuffer ASurfaceTransaction_setBuffer_ = nullptr;
    pASurfaceTransaction_apply ASurfaceTransaction_apply_ = nullptr;
    pASurfaceTransaction_delete ASurfaceTransaction_delete_ = nullptr;
};
