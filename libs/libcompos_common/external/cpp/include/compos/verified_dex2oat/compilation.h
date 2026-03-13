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

#pragma once

#include <android-base/result.h>

#include <memory>
#include <string_view>
#include <vector>

#include "verified_dex2oat/verified_dex2oat_compilation.h"

namespace compos {
// A wrapper around the AVerifiedDex2Oat_CompilationContext API.
class SecureCompilationContextInterface {
public:
    struct SuccessMetrics {
        uint32_t cpu_time_ms;
        uint32_t wall_time_ms;
    };

    struct FailureMetrics {
        uint32_t cpu_time_ms;
        uint32_t wall_time_ms;
        int32_t exit_code;
        uint32_t signal;
        AVerifiedDex2Oat_FailureReason reason_code;
        std::string_view reason;  // lifetime is equal to lifetime of associated FailureContext.
        std::string_view message; // lifetime is equal to lifetime of associated FailureContext.
    };

    virtual ~SecureCompilationContextInterface() = default;

    virtual android::base::Result<void> AddArg(std::string_view arg,
                                               const std::vector<int>& fds) = 0;

    virtual android::base::Result<void> StartCompilation(
            std::function<void(const AVerifiedDex2Oat_SuccessData*)>* on_success_cb,
            std::function<void(const AVerifiedDex2Oat_FailureData*)>* on_failure_cb,
            int32_t manifest_fd, uint32_t timeout_seconds) = 0;

    virtual android::base::Result<void> Cancel() = 0;

    virtual android::base::Result<SuccessMetrics> GetSuccessMetrics(
            const AVerifiedDex2Oat_SuccessData* result_ctx) = 0;

    virtual android::base::Result<FailureMetrics> GetFailureMetrics(
            const AVerifiedDex2Oat_FailureData* result_ctx) = 0;
};

class SecureCompilationContext : public SecureCompilationContextInterface {
public:
    ~SecureCompilationContext() override;

    static std::string_view StatusToString(AVerifiedDex2Oat_Status status);

    static android::base::Result<std::unique_ptr<SecureCompilationContext>> Create(
            int timeout_seconds);

    android::base::Result<void> AddArg(std::string_view arg, const std::vector<int>& fds) override;

    android::base::Result<void> StartCompilation(
            std::function<void(const AVerifiedDex2Oat_SuccessData*)>* on_success_cb,
            std::function<void(const AVerifiedDex2Oat_FailureData*)>* on_failure_cb,
            int32_t manifest_fd, uint32_t timeout_seconds) override;

    android::base::Result<void> Cancel() override;

    android::base::Result<SuccessMetrics> GetSuccessMetrics(
            const AVerifiedDex2Oat_SuccessData* result_ctx) override;

    android::base::Result<FailureMetrics> GetFailureMetrics(
            const AVerifiedDex2Oat_FailureData* result_ctx) override;

private:
    explicit SecureCompilationContext(AVerifiedDex2Oat_CompilationContext* compilation_context);
    AVerifiedDex2Oat_CompilationContext* compilation_context_;
};
} // namespace compos