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

#include "surface_control_dl.h"

#include <dlfcn.h>

#define LOAD_FUNCTION(lib, func)                                \
    do {                                                        \
        func##_ = reinterpret_cast<p##func>(dlsym(lib, #func)); \
        if (!func##_) {                                         \
            return false;                                       \
        }                                                       \
    } while (0)

SurfaceControl& SurfaceControl::GetInstance() {
    static android::base::NoDestructor<SurfaceControl> instance;
    return *instance;
}

SurfaceControl::SurfaceControl() {
    is_supported_ = LoadFunctions();
}

bool SurfaceControl::IsSupported() const {
    return is_supported_;
}

bool SurfaceControl::LoadFunctions() {
    void* lib = dlopen("libandroid.so", RTLD_NOW);
    if (lib == nullptr) {
        return false;
    }

    LOAD_FUNCTION(lib, ASurfaceControl_createFromWindow);
    LOAD_FUNCTION(lib, ASurfaceControl_release);
    LOAD_FUNCTION(lib, ASurfaceTransaction_create);
    LOAD_FUNCTION(lib, ASurfaceTransaction_setBuffer);
    LOAD_FUNCTION(lib, ASurfaceTransaction_apply);
    LOAD_FUNCTION(lib, ASurfaceTransaction_delete);

    return true;
}

ASurfaceControl* SurfaceControl::ASurfaceControl_createFromWindow(ANativeWindow* window,
                                                                  const char* debug_name) {
    return ASurfaceControl_createFromWindow_(window, debug_name);
}

void SurfaceControl::ASurfaceControl_release(ASurfaceControl* surface_control) {
    ASurfaceControl_release_(surface_control);
}

ASurfaceTransaction* SurfaceControl::ASurfaceTransaction_create() {
    return ASurfaceTransaction_create_();
}

void SurfaceControl::ASurfaceTransaction_setBuffer(ASurfaceTransaction* transaction,
                                                   ASurfaceControl* surface_control,
                                                   AHardwareBuffer* buffer,
                                                   int32_t acquire_fence_fd) {
    ASurfaceTransaction_setBuffer_(transaction, surface_control, buffer, acquire_fence_fd);
}

void SurfaceControl::ASurfaceTransaction_apply(ASurfaceTransaction* transaction) {
    ASurfaceTransaction_apply_(transaction);
}

void SurfaceControl::ASurfaceTransaction_delete(ASurfaceTransaction* transaction) {
    ASurfaceTransaction_delete_(transaction);
}