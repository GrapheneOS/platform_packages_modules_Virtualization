/*
 * Copyright 2024 The Android Open Source Project
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

#include <aidl/android/crosvm/BnCrosvmAndroidDisplayService.h>
#include <aidl/android/system/virtualizationservice_internal/IVirtualizationServiceInternal.h>
#include <android/binder_manager.h>
#include <android/binder_process.h>
#include <system/graphics.h> // for HAL_PIXEL_FORMAT_*

#include <condition_variable>
#include <memory>
#include <mutex>

using aidl::android::crosvm::BnCrosvmAndroidDisplayService;
using aidl::android::system::virtualizationservice_internal::IVirtualizationServiceInternal;
using aidl::android::view::Surface;

namespace {

class DisplayService : public BnCrosvmAndroidDisplayService {
public:
    DisplayService() = default;
    virtual ~DisplayService() = default;

    ndk::ScopedAStatus setSurface(Surface* surface, bool forCursor) override {
        {
            std::lock_guard lk(mSurfaceReadyMutex);
            if (forCursor) {
                mCursorSurface = std::make_unique<Surface>(surface->release());
            } else {
                mSurface = std::make_unique<Surface>(surface->release());
            }
        }
        mSurfaceReady.notify_all();
        return ::ndk::ScopedAStatus::ok();
    }

    ndk::ScopedAStatus removeSurface(bool forCursor) override {
        {
            std::lock_guard lk(mSurfaceReadyMutex);
            if (forCursor) {
                mCursorSurface = nullptr;
            } else {
                mSurface = nullptr;
            }
        }
        mSurfaceReady.notify_all();
        return ::ndk::ScopedAStatus::ok();
    }

    Surface* getSurface(bool forCursor) {
        std::unique_lock lk(mSurfaceReadyMutex);
        if (forCursor) {
            mSurfaceReady.wait(lk, [this] { return mCursorSurface != nullptr; });
            return mCursorSurface.get();
        } else {
            mSurfaceReady.wait(lk, [this] { return mSurface != nullptr; });
            return mSurface.get();
        }
    }
    ndk::ScopedFileDescriptor& getCursorStream() { return mCursorStream; }
    ndk::ScopedAStatus setCursorStream(const ndk::ScopedFileDescriptor& in_stream) {
        mCursorStream = ndk::ScopedFileDescriptor(dup(in_stream.get()));
        return ::ndk::ScopedAStatus::ok();
    }

private:
    std::condition_variable mSurfaceReady;
    std::mutex mSurfaceReadyMutex;
    std::unique_ptr<Surface> mSurface;
    std::unique_ptr<Surface> mCursorSurface;
    ndk::ScopedFileDescriptor mCursorStream;
};

} // namespace

typedef void (*ErrorCallback)(const char* message);

struct AndroidDisplayContext {
    std::shared_ptr<IVirtualizationServiceInternal> virt_service;
    std::shared_ptr<DisplayService> disp_service;
    ErrorCallback error_callback;

    AndroidDisplayContext(ErrorCallback cb) : error_callback(cb) {
        auto disp_service = ::ndk::SharedRefBase::make<DisplayService>();

        // Creates DisplayService and register it to the virtualizationservice. This is needed
        // because this code is executed inside of crosvm which runs as an app. Apps are not allowed
        // to register a service to the service manager.
        auto virt_service = IVirtualizationServiceInternal::fromBinder(ndk::SpAIBinder(
                AServiceManager_waitForService("android.system.virtualizationservice")));
        if (virt_service == nullptr) {
            errorf("Failed to find virtualization service");
            return;
        }
        auto status = virt_service->setDisplayService(disp_service->asBinder());
        if (!status.isOk()) {
            errorf("Failed to register display service");
            return;
        }

        this->virt_service = virt_service;
        this->disp_service = disp_service;
        ABinderProcess_startThreadPool();
    }

    ~AndroidDisplayContext() {
        if (virt_service == nullptr) {
            errorf("Not connected to virtualization service");
            return;
        }
        auto status = this->virt_service->clearDisplayService();
        if (!status.isOk()) {
            errorf("Failed to clear display service");
        }
    }

    void errorf(const char* format, ...) {
        char buffer[1024];

        va_list vararg;
        va_start(vararg, format);
        vsnprintf(buffer, sizeof(buffer), format, vararg);
        va_end(vararg);

        error_callback(buffer);
    }
};

extern "C" struct AndroidDisplayContext* create_android_display_context(
        const char*, ErrorCallback error_callback) {
    return new AndroidDisplayContext(error_callback);
}

extern "C" void destroy_android_display_context(struct AndroidDisplayContext* ctx) {
    delete ctx;
}

extern "C" ANativeWindow* create_android_surface(struct AndroidDisplayContext* ctx, uint32_t width,
                                                 uint32_t height, bool for_cursor) {
    if (ctx->disp_service == nullptr) {
        ctx->errorf("Display service was not created");
        return nullptr;
    }
    // Note: crosvm always uses BGRA8888 or BGRX8888. See devices/src/virtio/gpu/mod.rs in crosvm
    // where the SetScanoutBlob command is handled. Let's use BGRA not BGRX with a hope that we will
    // need alpha blending for the cursor surface.
    int format = HAL_PIXEL_FORMAT_BGRA_8888;
    ANativeWindow* surface = ctx->disp_service->getSurface(for_cursor)->get(); // this can block
    if (ANativeWindow_setBuffersGeometry(surface, width, height, format) != 0) {
        ctx->errorf("Failed to set buffer gemoetry");
        return nullptr;
    }
    // TODO(b/332785161): if we know that surface can get destroyed dynamically while VM is running,
    // consider calling ANativeWindow_acquire here and _release in destroy_android_surface, so that
    // crosvm doesn't hold a dangling pointer.
    return surface;
}

extern "C" void destroy_android_surface(struct AndroidDisplayContext*, ANativeWindow*) {
    // NOT IMPLEMENTED
}

extern "C" bool get_android_surface_buffer(struct AndroidDisplayContext* ctx,
                                           ANativeWindow* surface,
                                           ANativeWindow_Buffer* out_buffer) {
    if (out_buffer == nullptr) {
        ctx->errorf("out_buffer is null");
        return false;
    }
    if (ANativeWindow_lock(surface, out_buffer, nullptr) != 0) {
        ctx->errorf("Failed to lock buffer");
        return false;
    }
    return true;
}

extern "C" void set_android_surface_position(struct AndroidDisplayContext* ctx, uint32_t x,
                                             uint32_t y) {
    if (ctx->disp_service == nullptr) {
        ctx->errorf("Display service was not created");
        return;
    }
    auto fd = ctx->disp_service->getCursorStream().get();
    if (fd == -1) {
        ctx->errorf("Invalid fd");
        return;
    }
    uint32_t pos[] = {x, y};
    write(fd, pos, sizeof(pos));
}

extern "C" void post_android_surface_buffer(struct AndroidDisplayContext* ctx,
                                            ANativeWindow* surface) {
    if (ANativeWindow_unlockAndPost(surface) != 0) {
        ctx->errorf("Failed to unlock and post surface.");
        return;
    }
}
