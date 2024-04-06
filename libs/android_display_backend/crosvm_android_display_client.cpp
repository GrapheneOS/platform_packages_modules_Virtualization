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
#include <android/native_window.h>
#include <android/native_window_aidl.h>
#include <libyuv.h>
#include <stdint.h>
#include <utils/Errors.h>

#include <condition_variable>
#include <memory>
#include <mutex>
#include <vector>

using aidl::android::system::virtualizationservice_internal::IVirtualizationServiceInternal;

#define LIBEXPORT __attribute__((visibility("default"))) extern "C"

typedef void (*android_display_log_callback_type)(const char* message);

static void android_display_log_callback_stub(const char* message) {
    (void)message;
}

namespace {

class DisplayService : public aidl::android::crosvm::BnCrosvmAndroidDisplayService {
public:
    DisplayService() = default;
    virtual ~DisplayService() = default;

    ndk::ScopedAStatus setSurface(aidl::android::view::Surface* surface) override {
        {
            std::lock_guard lk(mSurfaceReadyMutex);
            mSurface = std::make_unique<aidl::android::view::Surface>(surface->release());
        }
        mSurfaceReady.notify_one();
        return ::ndk::ScopedAStatus::ok();
    }

    ndk::ScopedAStatus removeSurface() override {
        {
            std::lock_guard lk(mSurfaceReadyMutex);
            mSurface = nullptr;
        }
        mSurfaceReady.notify_one();
        return ::ndk::ScopedAStatus::ok();
    }

    aidl::android::view::Surface* getSurface() {
        std::unique_lock lk(mSurfaceReadyMutex);
        mSurfaceReady.wait(lk, [this] { return mSurface != nullptr; });
        return mSurface.get();
    }

private:
    std::condition_variable mSurfaceReady;
    std::mutex mSurfaceReadyMutex;
    std::unique_ptr<aidl::android::view::Surface> mSurface;
};

void ErrorF(android_display_log_callback_type error_callback, const char* format, ...) {
    char buffer[1024];

    va_list vararg;
    va_start(vararg, format);
    vsnprintf(buffer, sizeof(buffer), format, vararg);
    va_end(vararg);

    error_callback(buffer);
}

} // namespace

struct android_display_context {
    uint32_t width;
    uint32_t height;
    std::shared_ptr<DisplayService> displayService;
};

LIBEXPORT
struct android_display_context* create_android_display_context(
        const char* name, size_t name_len, android_display_log_callback_type error_callback) {
    auto ctx = new android_display_context();

    auto service = ::ndk::SharedRefBase::make<DisplayService>();

    if (strlen(name) != name_len) {
        ErrorF(error_callback, "Invalid service name length. Expected %u, actual %u", name_len,
               strlen(name));
        return nullptr;
    }
    ::ndk::SpAIBinder binder(
            AServiceManager_waitForService("android.system.virtualizationservice"));

    auto virt_service = IVirtualizationServiceInternal::fromBinder(binder);
    if (virt_service == nullptr) {
        ErrorF(error_callback, "Failed to find android.system.virtualizationservice");
        return nullptr;
    }
    auto status = virt_service->setDisplayService(service->asBinder());
    if (!status.isOk()) {
        ErrorF(error_callback, "Failed to register %s",
               aidl::android::crosvm::ICrosvmAndroidDisplayService::descriptor);
        return nullptr;
    }

    ABinderProcess_startThreadPool();

    auto surface = service->getSurface();
    ctx->width = static_cast<uint32_t>(ANativeWindow_getWidth(surface->get()));
    ctx->height = static_cast<uint32_t>(ANativeWindow_getHeight(surface->get()));
    ctx->displayService = service;
    return ctx;
}

LIBEXPORT
void destroy_android_display_context(android_display_log_callback_type error_callback,
                                     struct android_display_context* ctx) {
    auto service = ::ndk::SharedRefBase::make<DisplayService>();
    ::ndk::SpAIBinder binder(
            AServiceManager_waitForService("android.system.virtualizationservice"));
    auto virt_service = IVirtualizationServiceInternal::fromBinder(binder);
    if (virt_service != nullptr) {
        auto status = virt_service->clearDisplayService();
    } else {
        ErrorF(error_callback, "Failed to find android.system.virtualizationservice");
    }

    if (!ctx) {
        ErrorF(error_callback, "Invalid context.");
        return;
    }

    delete ctx;
}

LIBEXPORT
uint32_t get_android_display_width(android_display_log_callback_type error_callback,
                                   struct android_display_context* ctx) {
    if (!ctx) {
        ErrorF(error_callback, "Invalid context.");
        return -1;
    }
    if (!ctx->displayService->getSurface()) {
        ErrorF(error_callback, "Invalid context surface for ctx:%p.", ctx);
        return -1;
    }
    return ctx->width;
}

LIBEXPORT
uint32_t get_android_display_height(android_display_log_callback_type error_callback,
                                    struct android_display_context* ctx) {
    if (!ctx) {
        ErrorF(error_callback, "Invalid context.");
        return -1;
    }
    if (!ctx->displayService->getSurface()) {
        ErrorF(error_callback, "Invalid context surface for ctx:%p.", ctx);
        return -1;
    }
    return ctx->height;
}

uint16_t RGBA8888ToRGB565(uint8_t r, uint8_t g, uint8_t b, uint8_t a) {
    (void)a;
    return (static_cast<uint16_t>(r >> 3) << 11) | (static_cast<uint16_t>(g >> 2) << 5) |
            (static_cast<uint16_t>(b >> 3) << 0);
}

LIBEXPORT
void blit_android_display(android_display_log_callback_type error_callback,
                          struct android_display_context* ctx, uint32_t width, uint32_t height,
                          uint8_t* pixels, size_t pixels_num_bytes) {
    if (!ctx) {
        ErrorF(error_callback, "Invalid context.");
        return;
    }
    if (!ctx->displayService->getSurface()) {
        ErrorF(error_callback, "Invalid context surface.");
        return;
    }
    if (pixels_num_bytes != width * height * 4) {
        ErrorF(error_callback, "Invalid buffer size.");
        return;
    }
    ANativeWindow* anw = ctx->displayService->getSurface()->get();
    if (!anw) {
        ErrorF(error_callback, "Invalid context surface.");
        return;
    }

    ANativeWindow_Buffer anwBuffer = {};
    if (ANativeWindow_lock(anw, &anwBuffer, nullptr) != android::OK) {
        ErrorF(error_callback, "Failed to lock ANativeWindow.");
        return;
    }

    // Source is always BGRA8888.
    auto* src = reinterpret_cast<uint32_t*>(pixels);
    auto srcWidth = static_cast<uint32_t>(width);
    auto srcHeight = static_cast<uint32_t>(height);
    auto srcStrideBytes = srcWidth * 4;
    auto srcStridePixels = srcWidth;

    auto dstWidth = static_cast<uint32_t>(anwBuffer.width);
    auto dstHeight = static_cast<uint32_t>(anwBuffer.height);

    // Scale to fit if needed.
    std::vector<uint32_t> scaledSrc;
    if (srcWidth != dstWidth || srcHeight != dstHeight) {
        const float ratioWidth = static_cast<float>(dstWidth) / static_cast<float>(srcWidth);
        const float ratioHeight = static_cast<float>(dstHeight) / static_cast<float>(srcHeight);
        const float ratioUsed = std::min(ratioWidth, ratioHeight);

        uint32_t scaledSrcWidth = static_cast<uint32_t>(static_cast<float>(srcWidth) * ratioUsed);
        uint32_t scaledSrcHeight = static_cast<uint32_t>(static_cast<float>(srcHeight) * ratioUsed);
        uint32_t scaledSrcStrideBytes = scaledSrcWidth * 4;
        uint32_t scaledSrcStridePixels = scaledSrcWidth;

        scaledSrc.resize(scaledSrcHeight * scaledSrcStridePixels);

        libyuv::ARGBScale(reinterpret_cast<uint8_t*>(src), srcStrideBytes, srcWidth, srcHeight,
                          reinterpret_cast<uint8_t*>(scaledSrc.data()), scaledSrcStrideBytes,
                          scaledSrcWidth, scaledSrcHeight, libyuv::kFilterBilinear);

        src = scaledSrc.data();
        srcWidth = scaledSrcWidth;
        srcHeight = scaledSrcHeight;
        srcStrideBytes = scaledSrcStrideBytes;
        srcStridePixels = scaledSrcStridePixels;
    }

    if (anwBuffer.format == AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM) {
        auto* dst = reinterpret_cast<uint32_t*>(anwBuffer.bits);
        auto dstStridePixels = static_cast<uint32_t>(anwBuffer.stride);

        for (uint32_t h = 0; h < std::min(srcHeight, dstHeight); h++) {
            for (uint32_t w = 0; w < std::min(srcWidth, dstWidth); w++) {
                dst[(h * dstStridePixels) + w] = src[(h * srcStridePixels) + w];
            }
        }
    } else if (anwBuffer.format == AHARDWAREBUFFER_FORMAT_R5G6B5_UNORM) {
        auto* dst = reinterpret_cast<uint16_t*>(anwBuffer.bits);
        auto dstWidth = static_cast<uint32_t>(anwBuffer.width);
        auto dstHeight = static_cast<uint32_t>(anwBuffer.height);
        auto dstStridePixels = static_cast<uint32_t>(anwBuffer.stride);

        for (uint32_t h = 0; h < std::min(srcHeight, dstHeight); h++) {
            for (uint32_t w = 0; w < std::min(srcWidth, dstWidth); w++) {
                uint32_t srcPixel = src[(h * srcStridePixels) + w];
                uint8_t* srcPixelBytes = reinterpret_cast<uint8_t*>(&srcPixel);
                uint8_t r = srcPixelBytes[2];
                uint8_t g = srcPixelBytes[1];
                uint8_t b = srcPixelBytes[0];
                uint8_t a = srcPixelBytes[3];
                dst[(h * dstStridePixels) + w] = RGBA8888ToRGB565(r, g, b, a);
            }
        }
    } else {
        ErrorF(error_callback, "Unhandled format: %d", anwBuffer.format);
    }

    if (ANativeWindow_unlockAndPost(anw) != android::OK) {
        ErrorF(error_callback, "Failed to unlock and post ANativeWindow.");
        return;
    }
}
