/*
 * Copyright (C) 2026 The Android Open Source Project
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

#include <android-base/result.h>

#include "compos_wrappers/verified_dex2oat/compilation.h"

using android::base::Result;

namespace compos {
namespace {
void OnSuccess(const AVerifiedDex2Oat_SuccessData* result_ctx, void* user_callback) {
    SecureCompilationContextInterface::SuccessMetrics success_metric{};
    if (__builtin_available(android 38, *)) {
        success_metric.cpu_time_ms = AVerifiedDex2Oat_SuccessData_getCpuClockTimeMs(result_ctx);
        success_metric.wall_time_ms = AVerifiedDex2Oat_SuccessData_getWallClockTimeMs(result_ctx);
        auto success_cb = static_cast<
                std::function<void(const SecureCompilationContextInterface::SuccessMetrics&)>*>(
                user_callback);
        (*success_cb)(success_metric);
    }
}
void OnFailure(const AVerifiedDex2Oat_FailureData* result_ctx, void* user_callback) {
    if (__builtin_available(android 38, *)) {
        AVerifiedDex2Oat_FailureReason reason_code =
                AVerifiedDex2Oat_FailureData_getReason(result_ctx);
        Dex2OatStatus status = Dex2OatStatus::kUnknown;
        auto signal = AVerifiedDex2Oat_FailureData_getSignal(result_ctx);
        auto exit_code = AVerifiedDex2Oat_FailureData_getExitCode(result_ctx);
        if (reason_code == AVERIFIED_DEX2OAT_DEX2OAT_FAILED) {
            if (signal != 0) {
                status = Dex2OatStatus::kSignaled;
            } else if (exit_code != -1) {
                status = Dex2OatStatus::kExited;
            } else {
                status = Dex2OatStatus::kUnknown;
            }
        } else if (reason_code == AVERIFIED_DEX2OAT_COMPILATION_SETUP_FAILED) {
            status = Dex2OatStatus::kStartFailed;
        } else if (reason_code == AVERIFIED_DEX2OAT_TIMEOUT) {
            status = Dex2OatStatus::kTimedOut;
        } else if (reason_code == AVERIFIED_DEX2OAT_FAILURE_UNKNOWN) {
            status = Dex2OatStatus::kUnknown;
        }

        SecureCompilationContextInterface::FailureMetrics
                failure_metric{.status = status,
                               .cpu_time_ms =
                                       AVerifiedDex2Oat_FailureData_getCpuClockTimeMs(result_ctx),
                               .wall_time_ms =
                                       AVerifiedDex2Oat_FailureData_getWallClockTimeMs(result_ctx),
                               .exit_code = exit_code,
                               .signal = signal,
                               .message = AVerifiedDex2Oat_FailureData_getMessage(result_ctx)};
        auto failure_cb = static_cast<
                std::function<void(const SecureCompilationContextInterface::FailureMetrics&)>*>(
                user_callback);
        (*failure_cb)(failure_metric);
    } else {
        SecureCompilationContextInterface::FailureMetrics
                failure_metric{.status = Dex2OatStatus::kUnknown,
                               .cpu_time_ms = 0,
                               .wall_time_ms = 0,
                               .exit_code = -1,
                               .signal = 0,
                               .message = "API level is too low for secure compilation support."};
        auto failure_cb = static_cast<
                std::function<void(const SecureCompilationContextInterface::FailureMetrics&)>*>(
                user_callback);
        (*failure_cb)(failure_metric);
    }
}
} // namespace

SecureCompilationContext::SecureCompilationContext(
        AVerifiedDex2Oat_CompilationContext* compilation_context)
      : compilation_context_(compilation_context) {}

SecureCompilationContext::~SecureCompilationContext() {
    if (__builtin_available(android 38, *)) {
        AVerifiedDex2Oat_CompilationContext_destroy(compilation_context_);
    }
}

std::string_view SecureCompilationContext::StatusToString(AVerifiedDex2Oat_Status status) {
    if (__builtin_available(android 38, *)) {
        return AVerifiedDex2Oat_Status_toString(status);
    }
    return "API level is too low for secure compilation support.";
}

Result<std::unique_ptr<SecureCompilationContext>> SecureCompilationContext::Create(
        int timeout_seconds) {
    if (__builtin_available(android 38, *)) {
        AVerifiedDex2Oat_CompilationContext* compilation_context = nullptr;
        auto result =
                AVerifiedDex2Oat_CompilationContext_create(&compilation_context,
                                                           /*timeout_seconds=*/timeout_seconds);
        if (result != AVERIFIED_DEX2OAT_SUCCESS || compilation_context == nullptr) {
            return Errorf("Failed to create compilation context: {}", StatusToString(result));
        }
        return std::unique_ptr<SecureCompilationContext>(
                new SecureCompilationContext(compilation_context));
    }
    return Errorf("The current SDK is missing the VerifiedDex2Oat API.");
}

Result<void> SecureCompilationContext::AddArg(std::string_view arg, const std::vector<int>& fds) {
    if (__builtin_available(android 38, *)) {
        auto status =
                AVerifiedDex2Oat_CompilationContext_addArg(compilation_context_, arg.data(),
                                                           !fds.empty() ? fds.data() : nullptr,
                                                           !fds.empty() ? fds.size() : 0);
        if (status != AVERIFIED_DEX2OAT_SUCCESS) {
            return Errorf("Failed to add argument arg={}:{}", arg, StatusToString(status));
        }
        return {};
    }
    return Errorf("The current SDK is missing the VerifiedDex2Oat API.");
}

Result<void> SecureCompilationContext::StartCompilation(
        std::function<void(const SuccessMetrics&)>* success_user_data,
        std::function<void(const FailureMetrics&)>* failure_user_data, int32_t manifest_fd,
        uint32_t timeout_seconds) {
    if (__builtin_available(android 38, *)) {
        auto status =
                AVerifiedDex2Oat_start(compilation_context_, OnSuccess, success_user_data,
                                       OnFailure, failure_user_data, manifest_fd, timeout_seconds);
        if (status != AVERIFIED_DEX2OAT_SUCCESS) {
            return Errorf("Failed to start compilation: {}", StatusToString(status));
        }
        return {};
    }
    return Errorf("The current SDK is missing the VerifiedDex2Oat API.");
}

Result<void> SecureCompilationContext::Cancel() {
    if (__builtin_available(android 38, *)) {
        auto status = AVerifiedDex2Oat_cancel(compilation_context_);
        if (status != AVERIFIED_DEX2OAT_SUCCESS) {
            return Errorf("Failed to cancel compilation: {}", StatusToString(status));
        }
        return {};
    }
    return Errorf("The current SDK is missing the VerifiedDex2Oat API.");
}

} // namespace compos
