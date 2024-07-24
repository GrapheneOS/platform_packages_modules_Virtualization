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
#include <android-base/result.h>
#include <android/binder_manager.h>
#include <android/binder_process.h>
#include <system/graphics.h> // for HAL_PIXEL_FORMAT_*

#include <condition_variable>
#include <memory>
#include <mutex>

using aidl::android::crosvm::BnCrosvmAndroidDisplayService;
using aidl::android::system::virtualizationservice_internal::IVirtualizationServiceInternal;
using aidl::android::view::Surface;

using android::base::Error;
using android::base::Result;

namespace {

class SinkANativeWindow_Buffer {
public:
    Result<void> configure(uint32_t width, uint32_t height, int format) {
        if (format != HAL_PIXEL_FORMAT_BGRA_8888) {
            return Error() << "Pixel format " << format << " is not BGRA_8888.";
        }

        mBufferBits.resize(width * height * 4);
        mBuffer = ANativeWindow_Buffer{
                .width = static_cast<int32_t>(width),
                .height = static_cast<int32_t>(height),
                .stride = static_cast<int32_t>(width),
                .format = format,
                .bits = mBufferBits.data(),
        };
        return {};
    }

    operator ANativeWindow_Buffer&() { return mBuffer; }

private:
    ANativeWindow_Buffer mBuffer;
    std::vector<uint8_t> mBufferBits;
};

static Result<void> copyBuffer(ANativeWindow_Buffer& from, ANativeWindow_Buffer& to) {
    if (from.width != to.width || from.height != to.height) {
        return Error() << "dimension mismatch. from=(" << from.width << ", " << from.height << ") "
                       << "to=(" << to.width << ", " << to.height << ")";
    }
    uint32_t* dst = reinterpret_cast<uint32_t*>(to.bits);
    uint32_t* src = reinterpret_cast<uint32_t*>(from.bits);
    size_t bytes_on_line = to.width * 4; // 4 bytes per pixel
    for (int32_t h = 0; h < to.height; h++) {
        memcpy(dst + (h * to.stride), src + (h * from.stride), bytes_on_line);
    }
    return {};
}

// Wrapper which contains the latest available Surface/ANativeWindow from the DisplayService, if
// available. A Surface/ANativeWindow may not always be available if, for example, the VmLauncherApp
// on the other end of the DisplayService is not in the foreground / is paused.
class AndroidDisplaySurface {
public:
    AndroidDisplaySurface(const std::string& name) : mName(name) {}

    void setNativeSurface(Surface* surface) {
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

    Result<void> configure(uint32_t width, uint32_t height) {
        std::unique_lock lk(mSurfaceMutex);

        mRequestedSurfaceDimensions = Rect{
                .width = width,
                .height = height,
        };

        if (auto ret = mSinkBuffer.configure(width, height, kFormat); !ret.ok()) {
            return Error() << "Failed to configure sink buffer: " << ret.error();
        }
        if (auto ret = mSavedFrameBuffer.configure(width, height, kFormat); !ret.ok()) {
            return Error() << "Failed to configure saved frame buffer: " << ret.error();
        }
        return {};
    }

    void waitForNativeSurface() {
        std::unique_lock lk(mSurfaceMutex);
        mNativeSurfaceReady.wait(lk, [this] { return mNativeSurface != nullptr; });
    }

    Result<void> lock(ANativeWindow_Buffer* out_buffer) {
        std::unique_lock lk(mSurfaceMutex);

        Surface* surface = mNativeSurface.get();
        if (surface == nullptr) {
            // Surface not currently available but not necessarily an error
            // if, for example, the VmLauncherApp is not in the foreground.
            *out_buffer = mSinkBuffer;
            return {};
        }

        ANativeWindow* anw = surface->get();
        if (anw == nullptr) {
            return Error() << "Failed to get ANativeWindow";
        }

        if (mNativeSurfaceNeedsConfiguring) {
            if (!mRequestedSurfaceDimensions) {
                return Error() << "Surface dimension is not configured yet!";
            }
            const auto& dims = *mRequestedSurfaceDimensions;

            // Ensure locked buffers have our desired format.
            if (ANativeWindow_setBuffersGeometry(anw, dims.width, dims.height, kFormat) != 0) {
                return Error() << "Failed to set buffer geometry.";
            }

            mNativeSurfaceNeedsConfiguring = false;
        }

        if (ANativeWindow_lock(anw, out_buffer, nullptr) != 0) {
            return Error() << "Failed to lock window";
        }
        mLastBuffer = *out_buffer;
        return {};
    }

    Result<void> unlockAndPost() {
        std::unique_lock lk(mSurfaceMutex);

        Surface* surface = mNativeSurface.get();
        if (surface == nullptr) {
            // Surface not currently available but not necessarily an error
            // if, for example, the VmLauncherApp is not in the foreground.
            return {};
        }

        ANativeWindow* anw = surface->get();
        if (anw == nullptr) {
            return Error() << "Failed to get ANativeWindow";
        }

        if (ANativeWindow_unlockAndPost(anw) != 0) {
            return Error() << "Failed to unlock and post window";
        }
        return {};
    }

    // Saves the last frame drawn
    Result<void> saveFrame() {
        std::unique_lock lk(mSurfaceMutex);
        if (auto ret = copyBuffer(mLastBuffer, mSavedFrameBuffer); !ret.ok()) {
            return Error() << "Failed to copy frame: " << ret.error();
        }
        return {};
    }

    // Draws the saved frame
    Result<void> drawSavedFrame() {
        std::unique_lock lk(mSurfaceMutex);
        Surface* surface = mNativeSurface.get();
        if (surface == nullptr) {
            return Error() << "Surface not ready";
        }

        ANativeWindow* anw = surface->get();
        if (anw == nullptr) {
            return Error() << "Failed to get ANativeWindow";
        }

        // TODO: dedup this and the one in lock(...)
        if (mNativeSurfaceNeedsConfiguring) {
            if (!mRequestedSurfaceDimensions) {
                return Error() << "Surface dimension is not configured yet!";
            }
            const auto& dims = *mRequestedSurfaceDimensions;

            // Ensure locked buffers have our desired format.
            if (ANativeWindow_setBuffersGeometry(anw, dims.width, dims.height, kFormat) != 0) {
                return Error() << "Failed to set buffer geometry.";
            }

            mNativeSurfaceNeedsConfiguring = false;
        }

        ANativeWindow_Buffer buf;
        if (ANativeWindow_lock(anw, &buf, nullptr) != 0) {
            return Error() << "Failed to lock window";
        }

        if (auto ret = copyBuffer(mSavedFrameBuffer, buf); !ret.ok()) {
            return Error() << "Failed to copy frame: " << ret.error();
        }

        if (ANativeWindow_unlockAndPost(anw) != 0) {
            return Error() << "Failed to unlock and post window";
        }
        return {};
    }

    const std::string& name() const { return mName; }

private:
    // Note: crosvm always uses BGRA8888 or BGRX8888. See devices/src/virtio/gpu/mod.rs in
    // crosvm where the SetScanoutBlob command is handled. Let's use BGRA not BGRX with a hope
    // that we will need alpha blending for the cursor surface.
    static constexpr const int kFormat = HAL_PIXEL_FORMAT_BGRA_8888;

    std::string mName;

    std::mutex mSurfaceMutex;
    std::unique_ptr<Surface> mNativeSurface;
    std::condition_variable mNativeSurfaceReady;
    bool mNativeSurfaceNeedsConfiguring = true;

    // Buffer which crosvm uses when in background. This is just to not fail crosvm even when
    // Android-side Surface doesn't exist. The content drawn here is never displayed on the physical
    // screen.
    SinkANativeWindow_Buffer mSinkBuffer;

    // Buffer which is currently allocated for crosvm to draw onto. This holds the last frame. This
    // is what gets displayed on the physical screen.
    ANativeWindow_Buffer mLastBuffer;

    // Copy of mLastBuffer made by the call saveFrameForSurface. This holds the last good (i.e.
    // non-blank) frame before the VM goes background. When the VM is brought up to foreground,
    // this is drawn to the physical screen until the VM starts to emit actual frames.
    SinkANativeWindow_Buffer mSavedFrameBuffer;

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
        getSurface(forCursor).setNativeSurface(surface);
        return ::ndk::ScopedAStatus::ok();
    }

    ndk::ScopedAStatus removeSurface(bool forCursor) override {
        getSurface(forCursor).removeSurface();
        return ::ndk::ScopedAStatus::ok();
    }

    ndk::ScopedFileDescriptor& getCursorStream() { return mCursorStream; }
    ndk::ScopedAStatus setCursorStream(const ndk::ScopedFileDescriptor& in_stream) {
        mCursorStream = ndk::ScopedFileDescriptor(dup(in_stream.get()));
        return ::ndk::ScopedAStatus::ok();
    }

    ndk::ScopedAStatus saveFrameForSurface(bool forCursor) override {
        if (auto ret = getSurface(forCursor).saveFrame(); !ret.ok()) {
            std::string msg = std::format("Failed to save frame: {}", ret.error().message());
            return ::ndk::ScopedAStatus(
                    AStatus_fromServiceSpecificErrorWithMessage(-1, msg.c_str()));
        }
        return ::ndk::ScopedAStatus::ok();
    }

    ndk::ScopedAStatus drawSavedFrameForSurface(bool forCursor) override {
        if (auto ret = getSurface(forCursor).drawSavedFrame(); !ret.ok()) {
            std::string msg = std::format("Failed to draw saved frame: {}", ret.error().message());
            return ::ndk::ScopedAStatus(
                    AStatus_fromServiceSpecificErrorWithMessage(-1, msg.c_str()));
        }
        return ::ndk::ScopedAStatus::ok();
    }

    AndroidDisplaySurface& getSurface(bool forCursor) {
        if (forCursor) {
            return mCursor;
        } else {
            return mScanout;
        }
    }

private:
    AndroidDisplaySurface mScanout{"scanout"};
    AndroidDisplaySurface mCursor{"cursor"};
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

    AndroidDisplaySurface& surface = ctx->disp_service->getSurface(forCursor);
    if (auto ret = surface.configure(width, height); !ret.ok()) {
        ctx->errorf("Failed to configure surface %s: %s", surface.name().c_str(),
                    ret.error().message().c_str());
    }

    surface.waitForNativeSurface(); // this can block

    // TODO(b/332785161): if we know that surface can get destroyed dynamically while VM is running,
    // consider calling ANativeWindow_acquire here and _release in destroy_android_surface, so that
    // crosvm doesn't hold a dangling pointer.
    return &surface;
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

    auto ret = surface->lock(out_buffer);
    if (!ret.ok()) {
        ctx->errorf("Failed to lock surface %s: %s", surface->name().c_str(),
                    ret.error().message().c_str());
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

    auto ret = surface->unlockAndPost();
    if (!ret.ok()) {
        ctx->errorf("Failed to unlock and post for surface %s: %s", surface->name().c_str(),
                    ret.error().message().c_str());
    }
    return;
}
