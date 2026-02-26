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
#include <cutils/native_handle.h>
#include <system/graphics.h> // for HAL_PIXEL_FORMAT_*
#include <vndk/hardware_buffer.h>

#include <condition_variable>
#include <memory>
#include <mutex>

#include "surface_control_dl.h"

using aidl::android::crosvm::BnCrosvmAndroidDisplayService;
using aidl::android::system::virtualizationservice_internal::IVirtualizationServiceInternal;
using aidl::android::view::Surface;

using android::base::Error;
using android::base::Result;

namespace {
class AndroidDisplaySurface {
public:
    AndroidDisplaySurface(const std::string& name) : mName(name) {}

    Result<void> setNativeSurface(const Surface& surface) {
        {
            std::lock_guard lk(mSurfaceMutex);
            mNativeSurface = std::make_unique<Surface>();
            mNativeSurface->reset(surface.get());
            Surface* native_surface = mNativeSurface.get();
            if (!native_surface) {
                return Error() << "Failed to get Surface";
            }

            ANativeWindow* anw = native_surface->get();
            auto& sc = SurfaceControl::GetInstance();
            if (sc.IsSupported()) {
                mSurfaceControl = sc.ASurfaceControl_createFromWindow(anw, mName.c_str());
            }
            if (!mSurfaceControl) {
                return Error() << "Failed to create ASurfaceControl";
            }

            if (mLastAHB) {
                setBufferLocked(mLastAHB);
            }
        }

        mNativeSurfaceReady.notify_one();
        return {};
    }

    void removeSurface() {
        {
            std::lock_guard lk(mSurfaceMutex);
            auto& sc = SurfaceControl::GetInstance();
            if (mSurfaceControl) {
                if (sc.IsSupported()) sc.ASurfaceControl_release(mSurfaceControl);
                mSurfaceControl = nullptr;
            }
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

        if (mSoftwareAHB != nullptr) {
            AHardwareBuffer_Desc desc;
            AHardwareBuffer_describe(mSoftwareAHB, &desc);
            if (desc.width == width && desc.height == height) {
                // Keep the existing AHB
                return {};
            } else {
                AHardwareBuffer_release(mSoftwareAHB);
                mSoftwareAHB = nullptr;
            }
        }

        if (mSoftwareAHB == nullptr) {
            AHardwareBuffer_Desc desc = {
                    .width = width,
                    .height = height,
                    .layers = 1,
                    .format = AHARDWAREBUFFER_FORMAT_B8G8R8A8_UNORM,
                    .usage = AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN |
                            AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE,
            };
            int status = AHardwareBuffer_allocate(&desc, &mSoftwareAHB);
            if (status != 0 || mSoftwareAHB == nullptr) {
                return Error() << "Failed to allocate AHardwareBuffer: " << status;
            }
        }

        return {};
    }

    void waitForNativeSurface() {
        std::unique_lock lk(mSurfaceMutex);
        mNativeSurfaceReady.wait(lk, [this] { return mNativeSurface != nullptr; });
    }

    Result<void> lock(ANativeWindow_Buffer* out_buffer) {
        std::unique_lock lk(mSurfaceMutex);

        if (!mSoftwareAHB) {
            return Error() << "mSoftwareAHB is not allocated";
        }

        void* bits = nullptr;
        // Lock the AHardwareBuffer for CPU write
        int status = AHardwareBuffer_lock(mSoftwareAHB, AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN, -1,
                                          nullptr, &bits);
        if (status != 0) {
            return Error() << "Failed to lock AHardwareBuffer: " << status;
        }

        AHardwareBuffer_Desc desc;
        AHardwareBuffer_describe(mSoftwareAHB, &desc);

        out_buffer->width = desc.width;
        out_buffer->height = desc.height;
        out_buffer->stride = desc.stride;
        out_buffer->format = desc.format;
        out_buffer->bits = bits;

        return {};
    }

    Result<void> unlockAndPost() {
        std::unique_lock lk(mSurfaceMutex);
        if (!mSoftwareAHB) {
            return Error() << "mSoftwareAHB is not allocated";
        }

        int status = AHardwareBuffer_unlock(mSoftwareAHB, nullptr);
        if (status != 0) {
            return Error() << "Failed to unlock AHardwareBuffer: " << status;
        }

        Surface* surface = mNativeSurface.get();
        if (surface == nullptr) {
            // If the surface is not ready yet, we just updated the AHB content successfully.
            // We can drop the setBuffer for now, the UI might be backgrounded.
            return {};
        }

        return setBufferLocked(mSoftwareAHB);
    }

    Result<void> setBuffer(AHardwareBuffer* ahb) {
        std::lock_guard lk(mSurfaceMutex);
        return setBufferLocked(ahb);
    }

    const std::string& name() const { return mName; }

private:
    Result<void> setBufferLocked(AHardwareBuffer* ahb) {
        setLastAHB(ahb);

        auto& sc = SurfaceControl::GetInstance();
        if (!sc.IsSupported()) {
            return Error() << "SurfaceControl is not supported";
        }
        auto transaction = sc.ASurfaceTransaction_create();
        if (!transaction) {
            return Error() << "Failed to create ASurfaceTransaction";
        }
        if (!mSurfaceControl) {
            // Silently drop the frame when the UI is in the background
            return {};
        }

        sc.ASurfaceTransaction_setBuffer(transaction, mSurfaceControl, ahb,
                                         -1 /* acquire_fence_fd */);
        sc.ASurfaceTransaction_apply(transaction);
        sc.ASurfaceTransaction_delete(transaction);

        return {};
    }

    void setLastAHB(AHardwareBuffer* ahb) {
        if (mLastAHB == ahb) return;

        if (ahb) AHardwareBuffer_acquire(ahb);
        if (mLastAHB) AHardwareBuffer_release(mLastAHB);

        mLastAHB = ahb;
    }

    // Note: crosvm always uses BGRA8888 or BGRX8888. See devices/src/virtio/gpu/mod.rs in
    // crosvm where the SetScanoutBlob command is handled. Let's use BGRA not BGRX with a hope
    // that we will need alpha blending for the cursor surface.
    static constexpr const int kFormat = HAL_PIXEL_FORMAT_BGRA_8888;

public:
    ~AndroidDisplaySurface() {
        if (mLastAHB) {
            AHardwareBuffer_release(mLastAHB);
        }
        if (mSoftwareAHB) {
            AHardwareBuffer_release(mSoftwareAHB);
        }
    }
    std::string mName;

    std::mutex mSurfaceMutex;
    std::unique_ptr<Surface> mNativeSurface;
    ASurfaceControl* mSurfaceControl = nullptr;
    std::condition_variable mNativeSurfaceReady;

    AHardwareBuffer* mSoftwareAHB = nullptr;
    AHardwareBuffer* mLastAHB = nullptr;
};

class DisplayService : public BnCrosvmAndroidDisplayService {
public:
    DisplayService() = default;
    virtual ~DisplayService() = default;

    ndk::ScopedAStatus setSurface(const Surface& surface, bool forCursor) override {
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

struct AHardwareBufferInfo {
    size_t num_fds;
    const int32_t* fds_ptr;
    size_t metadata_len;
    const uint8_t* metadata_ptr;

    // The logic should be synced with frameworks/native/libs/nativewindow/rust/src/lib.rs
    Result<AHardwareBuffer*> toAHardwareBuffer() const {
        if (metadata_len < sizeof(AHardwareBuffer_Desc)) {
            return Error()
                    << "Metadata size should be greater than the size of AHardwareBuffer_Desc";
        }

        AHardwareBuffer_Desc desc;
        memcpy(&desc, metadata_ptr, sizeof(AHardwareBuffer_Desc));

        size_t ints_size = metadata_len - sizeof(AHardwareBuffer_Desc);
        int numInts = ints_size / sizeof(int);

        native_handle_t* handle = native_handle_create(num_fds, numInts);
        if (!handle) {
            return Error() << "Failed to create native_handle";
        }

        for (size_t i = 0; i < num_fds; ++i) {
            handle->data[i] = fds_ptr[i];
        }

        memcpy(&handle->data[num_fds], metadata_ptr + sizeof(AHardwareBuffer_Desc), ints_size);

        AHardwareBuffer* ahb = nullptr;
        int status =
                AHardwareBuffer_createFromHandle(&desc, handle,
                                                 AHARDWAREBUFFER_CREATE_FROM_HANDLE_METHOD_CLONE,
                                                 &ahb);

        native_handle_delete(handle);

        if (status != 0) {
            return Error() << "Failed to create AHB from handle: " << status;
        }

        return ahb;
    }
};

extern "C" void android_display_flip_to(struct AndroidDisplayContext* ctx,
                                        AndroidDisplaySurface* surface,
                                        const AHardwareBufferInfo* info) {
    if (info == nullptr) {
        ctx->errorf("Invalid AHardwareBufferInfo provided");
        return;
    }

    auto ahbResult = info->toAHardwareBuffer();
    if (!ahbResult.ok()) {
        ctx->errorf("%s", ahbResult.error().message().c_str());
        return;
    }
    AHardwareBuffer* ahb = *ahbResult;

    auto ret = surface->setBuffer(ahb);
    if (!ret.ok()) {
        ctx->errorf("Failed to set buffer %s: %s", surface->name().c_str(),
                    ret.error().message().c_str());
    }

    AHardwareBuffer_release(ahb);
}
