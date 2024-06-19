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

class SinkANativeWindow_Buffer {
public:
    SinkANativeWindow_Buffer() = default;
    virtual ~SinkANativeWindow_Buffer() = default;

    bool configure(uint32_t width, uint32_t height, int format) {
        if (format != HAL_PIXEL_FORMAT_BGRA_8888) {
            return false;
        }

        mBufferBits.resize(width * height * 4);
        mBuffer = ANativeWindow_Buffer{
                .width = static_cast<int32_t>(width),
                .height = static_cast<int32_t>(height),
                .stride = static_cast<int32_t>(width),
                .format = format,
                .bits = mBufferBits.data(),
        };
        return true;
    }

    operator ANativeWindow_Buffer&() { return mBuffer; }

private:
    ANativeWindow_Buffer mBuffer;
    std::vector<uint8_t> mBufferBits;
};

// Wrapper which contains the latest available Surface/ANativeWindow
// from the DisplayService, if available. A Surface/ANativeWindow may
// not always be available if, for example, the VmLauncherApp on the
// other end of the DisplayService is not in the foreground / is paused.
class AndroidDisplaySurface {
public:
    AndroidDisplaySurface() = default;
    virtual ~AndroidDisplaySurface() = default;

    void setSurface(Surface* surface) {
        {
            std::lock_guard lk(mSurfaceMutex);
            mNativeSurface = std::make_unique<Surface>(surface->release());
            mNativeSurfaceNeedsConfiguring = true;
        }

        mNativeSurfaceReady.notify_one();
    }

    void removeSurface() {
        {
            std::lock_guard lk(mSurfaceMutex);
            mNativeSurface = nullptr;
        }
        mNativeSurfaceReady.notify_one();
    }

    Surface* getSurface() {
        std::unique_lock lk(mSurfaceMutex);
        return mNativeSurface.get();
    }

    void configure(uint32_t width, uint32_t height) {
        std::unique_lock lk(mSurfaceMutex);

        mRequestedSurfaceDimensions = Rect{
                .width = width,
                .height = height,
        };

        mSinkBuffer.configure(width, height, kFormat);
    }

    void waitForNativeSurface() {
        std::unique_lock lk(mSurfaceMutex);
        mNativeSurfaceReady.wait(lk, [this] { return mNativeSurface != nullptr; });
    }

    int lock(ANativeWindow_Buffer* out_buffer) {
        std::unique_lock lk(mSurfaceMutex);

        Surface* surface = mNativeSurface.get();
        if (surface == nullptr) {
            // Surface not currently available but not necessarily an error
            // if, for example, the VmLauncherApp is not in the foreground.
            *out_buffer = mSinkBuffer;
            return 0;
        }

        ANativeWindow* anw = surface->get();
        if (anw == nullptr) {
            return -1;
        }

        if (mNativeSurfaceNeedsConfiguring) {
            if (!mRequestedSurfaceDimensions) {
                return -1;
            }
            const auto& dims = *mRequestedSurfaceDimensions;

            // Ensure locked buffers have our desired format.
            if (ANativeWindow_setBuffersGeometry(anw, dims.width, dims.height, kFormat) != 0) {
                return -1;
            }

            mNativeSurfaceNeedsConfiguring = false;
        }

        return ANativeWindow_lock(anw, out_buffer, nullptr);
    }

    int unlockAndPost() {
        std::unique_lock lk(mSurfaceMutex);

        Surface* surface = mNativeSurface.get();
        if (surface == nullptr) {
            // Surface not currently available but not necessarily an error
            // if, for example, the VmLauncherApp is not in the foreground.
            return 0;
        }

        ANativeWindow* anw = surface->get();
        if (anw == nullptr) {
            return -1;
        }

        return ANativeWindow_unlockAndPost(anw);
    }

private:
    // Note: crosvm always uses BGRA8888 or BGRX8888. See devices/src/virtio/gpu/mod.rs in
    // crosvm where the SetScanoutBlob command is handled. Let's use BGRA not BGRX with a hope
    // that we will need alpha blending for the cursor surface.
    static constexpr const int kFormat = HAL_PIXEL_FORMAT_BGRA_8888;

    std::mutex mSurfaceMutex;
    std::unique_ptr<Surface> mNativeSurface;
    std::condition_variable mNativeSurfaceReady;
    bool mNativeSurfaceNeedsConfiguring = true;

    SinkANativeWindow_Buffer mSinkBuffer;

    struct Rect {
        uint32_t width = 0;
        uint32_t height = 0;
    };
    std::optional<Rect> mRequestedSurfaceDimensions;
};

class DisplayService : public BnCrosvmAndroidDisplayService {
public:
    DisplayService() = default;
    virtual ~DisplayService() = default;

    ndk::ScopedAStatus setSurface(Surface* surface, bool forCursor) override {
        if (forCursor) {
            mCursor.setSurface(surface);
        } else {
            mScanout.setSurface(surface);
        }
        return ::ndk::ScopedAStatus::ok();
    }

    ndk::ScopedAStatus removeSurface(bool forCursor) override {
        if (forCursor) {
            mCursor.removeSurface();
        } else {
            mScanout.removeSurface();
        }
        return ::ndk::ScopedAStatus::ok();
    }

    AndroidDisplaySurface* getCursorSurface() { return &mCursor; }
    AndroidDisplaySurface* getScanoutSurface() { return &mScanout; }

    ndk::ScopedFileDescriptor& getCursorStream() { return mCursorStream; }
    ndk::ScopedAStatus setCursorStream(const ndk::ScopedFileDescriptor& in_stream) {
        mCursorStream = ndk::ScopedFileDescriptor(dup(in_stream.get()));
        return ::ndk::ScopedAStatus::ok();
    }

private:
    AndroidDisplaySurface mScanout;
    AndroidDisplaySurface mCursor;
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

extern "C" AndroidDisplaySurface* create_android_surface(struct AndroidDisplayContext* ctx,
                                                         uint32_t width, uint32_t height,
                                                         bool forCursor) {
    if (ctx->disp_service == nullptr) {
        ctx->errorf("Display service was not created");
        return nullptr;
    }

    AndroidDisplaySurface* displaySurface = forCursor ? ctx->disp_service->getCursorSurface()
                                                      : ctx->disp_service->getScanoutSurface();
    if (displaySurface == nullptr) {
        ctx->errorf("AndroidDisplaySurface was not created");
        return nullptr;
    }

    displaySurface->configure(width, height);

    displaySurface->waitForNativeSurface(); // this can block

    // TODO(b/332785161): if we know that surface can get destroyed dynamically while VM is running,
    // consider calling ANativeWindow_acquire here and _release in destroy_android_surface, so that
    // crosvm doesn't hold a dangling pointer.
    return displaySurface;
}

extern "C" void destroy_android_surface(struct AndroidDisplayContext*, ANativeWindow*) {
    // NOT IMPLEMENTED
}

extern "C" bool get_android_surface_buffer(struct AndroidDisplayContext* ctx,
                                           AndroidDisplaySurface* surface,
                                           ANativeWindow_Buffer* out_buffer) {
    if (out_buffer == nullptr) {
        ctx->errorf("out_buffer is null");
        return false;
    }

    if (surface == nullptr) {
        ctx->errorf("Invalid AndroidDisplaySurface provided");
        return false;
    }

    if (surface->lock(out_buffer) != 0) {
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
                                            AndroidDisplaySurface* surface) {
    if (surface == nullptr) {
        ctx->errorf("Invalid AndroidDisplaySurface provided");
        return;
    }

    if (surface->unlockAndPost() != 0) {
        ctx->errorf("Failed to unlock and post AndroidDisplaySurface.");
        return;
    }
}
