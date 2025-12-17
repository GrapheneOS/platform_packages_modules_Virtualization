/*
 * Copyright (C) 2025 The Android Open Source Project
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

#include <android-base/logging.h>
#include <fcntl.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <filesystem>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <vector>

#include "android-base/file.h"
#include "android-base/properties.h"
#include "android-base/stringprintf.h"
#include "android-base/strings.h"
#include "android-base/unique_fd.h"
#include "verified_dex2oat.h"

namespace compos_api {
namespace {

using ::TemporaryFile;

using ::testing::_;
using ::testing::TestWithParam;
using ::testing::ValuesIn;

using ::android::base::GetBoolProperty;
using ::android::base::GetProperty;
using ::android::base::StringPrintf;
using ::android::base::unique_fd;
namespace fs = std::filesystem;

constexpr int THREE_SECONDS{3};
constexpr int TEN_SECONDS{10};
constexpr int FIFTEEN_SECONDS{15};

class MockResultCallbacks {
public:
    MockResultCallbacks() : done_(false) {}

    void OnSuccess(const AVerifiedDex2Oat_SuccessData* ctx) {
        MockOnSuccess(ctx);
        SignalDone();
    }
    void OnFailure(const AVerifiedDex2Oat_FailureData* ctx) {
        MockOnFailure(ctx);
        SignalDone();
    }

    bool WaitUntilDoneWithTimeout(int timeout_seconds) {
        std::unique_lock<std::mutex> lock(mutex_);
        return cv_.wait_for(lock, std::chrono::seconds(timeout_seconds), [this] { return done_; });
    }

    void* AsUserData() { return reinterpret_cast<void*>(this); }

    MOCK_METHOD(void, MockOnSuccess, (const AVerifiedDex2Oat_SuccessData*));
    MOCK_METHOD(void, MockOnFailure, (const AVerifiedDex2Oat_FailureData*));

private:
    void SignalDone() {
        std::lock_guard<std::mutex> lock(mutex_);
        done_ = true;
        cv_.notify_one();
    }

    std::mutex mutex_;
    std::condition_variable cv_;
    bool done_;
};

void OnSuccessCb(const AVerifiedDex2Oat_SuccessData* success_data, void* user_data) {
    reinterpret_cast<MockResultCallbacks*>(user_data)->OnSuccess(success_data);
}

void OnFailureCb(const AVerifiedDex2Oat_FailureData* failure_data, void* user_data) {
    reinterpret_cast<MockResultCallbacks*>(user_data)->OnFailure(failure_data);
}

unique_fd OpenFileForRead(const char* path) {
    return unique_fd(open(path, O_RDONLY));
}

fs::path GetFsPath(const unique_fd& fd) {
    return fs::read_symlink(fs::path("/proc/self/fd") / std::to_string(fd.get()));
}

std::string GetPath(const unique_fd& fd) {
    return GetFsPath(fd).string();
}

std::string GetDir(const unique_fd& fd) {
    return GetFsPath(fd).parent_path().string();
}

} // namespace

class VerifiedDex2oatTest : public ::testing::Test {
protected:
    std::string GetPropertyOrEmpty(const char* property) { return GetProperty(property, ""); }

    void SetUp() override {
        mock_result_cbs_ = std::make_unique<MockResultCallbacks>();
        std::string arch = GetPropertyOrEmpty("ro.bionic.arch");
        isa_features_property_ = StringPrintf("dalvik.vm.isa.%s.features", arch.c_str()).c_str();
        isa_variant_property_ = StringPrintf("dalvik.vm.isa.%s.variant", arch.c_str()).c_str();
    }

    void TearDown() override {
        if (compilation_context_ != nullptr) {
            AVerifiedDex2Oat_CompilationContext_destroy(compilation_context_);
            compilation_context_ = nullptr;
        }
    }

    void AddIfPropertyIsNonEmpty(AVerifiedDex2Oat_CompilationContext* ctx, const char* fmt_string,
                                 const std::string& key) {
        std::string prop_val = GetPropertyOrEmpty(key.c_str());
        if (!prop_val.empty()) {
            AddArg(ctx, StringPrintf(fmt_string, prop_val.c_str()).c_str());
        }
    }

    void AddIfPropTrue(AVerifiedDex2Oat_CompilationContext* ctx, const char* fmt_string,
                       const std::string& key) {
        if (GetBoolProperty(key, false)) {
            AddArg(ctx, fmt_string);
        }
    }

    void AddPropOrError(AVerifiedDex2Oat_CompilationContext* ctx, const char* fmt_string,
                        const std::string& key) {
        std::string prop_val = GetPropertyOrEmpty(key.c_str());

        ASSERT_FALSE(prop_val.empty()) << "FormatString:" << fmt_string << " Property:" << key;
        AddArg(ctx, StringPrintf(fmt_string, prop_val.c_str()).c_str());
    }

    void AddArg(AVerifiedDex2Oat_CompilationContext* ctx, const char* format_str) {
        ASSERT_EQ(AVerifiedDex2Oat_CompilationContext_addArg(ctx, format_str, nullptr, 0),
                  AVERIFIED_DEX2OAT_SUCCESS);
    }

    void AddArg(AVerifiedDex2Oat_CompilationContext* ctx, const std::string& format_str) {
        AddArg(ctx, format_str.c_str());
    }

    void AddArgWithFds(AVerifiedDex2Oat_CompilationContext* ctx, const char* format_str,
                       std::vector<int> fds) {
        ASSERT_EQ(AVerifiedDex2Oat_CompilationContext_addArg(ctx, format_str, fds.data(),
                                                             fds.size()),
                  AVERIFIED_DEX2OAT_SUCCESS);
    }

    void GetPackagePath(const std::string& pname, std::string* result) {
        std::array<char, 128> buffer;
        std::string cmd = "pm path " + pname;
        std::unique_ptr<FILE, decltype(&pclose)> cmd_stdout(popen(cmd.c_str(), "r"), pclose);
        ASSERT_TRUE(cmd_stdout);
        std::string package_path;
        while (fgets(buffer.data(), buffer.size(), cmd_stdout.get()) != nullptr) {
            package_path += buffer.data();
        }
        const std::string prefix = "package:";
        ASSERT_EQ(package_path.rfind(prefix, 0), 0);
        package_path.erase(0, prefix.length());
        package_path.erase(std::remove(package_path.begin(), package_path.end(), '\n'),
                           package_path.end());
        package_path.erase(std::remove(package_path.begin(), package_path.end(), '\r'),
                           package_path.end());
        struct stat stat_buff;
        ASSERT_EQ(stat(package_path.c_str(), &stat_buff), 0);
        *result = std::move(package_path);
    }

    void AddArgsToContext(AVerifiedDex2Oat_CompilationContext* ctx) {
        std::string zip_location;
        GetPackagePath("com.android.compos.testapk", &zip_location);
        unique_fd zip_file = OpenFileForRead(zip_location.c_str());
        ASSERT_NE(zip_file.get(), -1);
        AddArgWithFds(ctx, "--zip-fd=!", {zip_file.get()});
        AddArg(ctx, "--zip-location=" + zip_location);
        TemporaryFile oat_file;
        AddArgWithFds(ctx, "--oat-fd=!", {oat_file.fd});
        AddArg(ctx, "--oat-location=" + std::string(oat_file.path));
        AddPropOrError(ctx, "--instruction-set=%s", "ro.bionic.arch");
        AddArg(ctx, "--compiler-filter=verify");
    }

    std::unique_ptr<MockResultCallbacks> mock_result_cbs_;
    AVerifiedDex2Oat_CompilationContext* compilation_context_ = nullptr;
    std::string isa_features_property_;
    std::string isa_variant_property_;
    std::string max_image_block_size_;
    TemporaryFile recorded_args_file_;
};

TEST_F(VerifiedDex2oatTest, VerifiedDex2OatSucceeds) {
    EXPECT_CALL(*mock_result_cbs_, MockOnSuccess(_))
            .WillOnce([](const AVerifiedDex2Oat_SuccessData* ctx) {
                EXPECT_GT(AVerifiedDex2Oat_SuccessData_getWallClockTimeMs(ctx), 0);
                EXPECT_GT(AVerifiedDex2Oat_SuccessData_getCpuClockTimeMs(ctx), 0);
            });
    EXPECT_EQ(AVerifiedDex2Oat_CompilationContext_create(&compilation_context_, THREE_SECONDS),
              AVERIFIED_DEX2OAT_SUCCESS);
    ASSERT_NE(compilation_context_, nullptr);
    AddArgsToContext(compilation_context_);
    EXPECT_EQ(AVerifiedDex2Oat_start(compilation_context_, &OnSuccessCb,
                                     mock_result_cbs_->AsUserData(), &OnFailureCb,
                                     mock_result_cbs_->AsUserData(), recorded_args_file_.fd,
                                     TEN_SECONDS),
              AVERIFIED_DEX2OAT_SUCCESS);
    mock_result_cbs_->WaitUntilDoneWithTimeout(FIFTEEN_SECONDS);
    AVerifiedDex2Oat_CompilationContext_destroy(compilation_context_);
    compilation_context_ = nullptr;
}

TEST_F(VerifiedDex2oatTest, VerifiedDex2OatFails) {
    EXPECT_CALL(*mock_result_cbs_, MockOnFailure(_))
            .WillOnce([](const AVerifiedDex2Oat_FailureData* ctx) {
                EXPECT_EQ(AVerifiedDex2Oat_FailureData_getReason(ctx),
                          AVERIFIED_DEX2OAT_DEX2OAT_FAILED);
                EXPECT_NE(AVerifiedDex2Oat_FailureData_getMessage(ctx), nullptr);
                EXPECT_NE(AVerifiedDex2Oat_FailureData_getExitCode(ctx), 0);
                (void)AVerifiedDex2Oat_FailureData_getCpuClockTimeMs(ctx);
                (void)AVerifiedDex2Oat_FailureData_getWallClockTimeMs(ctx);
                EXPECT_EQ(AVerifiedDex2Oat_FailureData_getSignal(ctx), 0);
            });
    EXPECT_CALL(*mock_result_cbs_, MockOnSuccess(_)).Times(0);
    EXPECT_EQ(AVerifiedDex2Oat_CompilationContext_create(&compilation_context_, THREE_SECONDS),
              AVERIFIED_DEX2OAT_SUCCESS);
    ASSERT_NE(compilation_context_, nullptr);
    AddArg(compilation_context_, "a bad arg");
    EXPECT_EQ(AVerifiedDex2Oat_start(compilation_context_, &OnSuccessCb,
                                     mock_result_cbs_->AsUserData(), &OnFailureCb,
                                     mock_result_cbs_->AsUserData(), recorded_args_file_.fd,
                                     TEN_SECONDS),
              AVERIFIED_DEX2OAT_SUCCESS);
    mock_result_cbs_->WaitUntilDoneWithTimeout(FIFTEEN_SECONDS);
    AVerifiedDex2Oat_CompilationContext_destroy(compilation_context_);
    compilation_context_ = nullptr;
}

TEST_F(VerifiedDex2oatTest, VerifiedDex2OatCancelsProperly) {
    EXPECT_CALL(*mock_result_cbs_, MockOnSuccess(_)).Times(0);
    EXPECT_CALL(*mock_result_cbs_, MockOnFailure(_)).Times(0);
    EXPECT_EQ(AVerifiedDex2Oat_CompilationContext_create(&compilation_context_, THREE_SECONDS),
              AVERIFIED_DEX2OAT_SUCCESS);
    ASSERT_NE(compilation_context_, nullptr);
    AddArg(compilation_context_, "a bad arg");
    EXPECT_EQ(AVerifiedDex2Oat_start(compilation_context_, &OnSuccessCb,
                                     mock_result_cbs_->AsUserData(), &OnFailureCb,
                                     mock_result_cbs_->AsUserData(), recorded_args_file_.fd,
                                     TEN_SECONDS),
              AVERIFIED_DEX2OAT_SUCCESS);
    EXPECT_EQ(AVerifiedDex2Oat_cancel(compilation_context_), AVERIFIED_DEX2OAT_SUCCESS);
    AVerifiedDex2Oat_CompilationContext_destroy(compilation_context_);

    compilation_context_ = nullptr;
}

struct FailureReasonToStringTestParam {
    AVerifiedDex2Oat_FailureReason reason;
    std::string expected_string;
};

class FailureReasonToStringTest : public TestWithParam<FailureReasonToStringTestParam> {};

TEST_P(FailureReasonToStringTest, Works) {
    const auto& [reason, expected_string] = GetParam();
    const char* result = AVerifiedDex2Oat_FailureReason_toString(reason);
    EXPECT_STREQ(result, expected_string.c_str());
}

INSTANTIATE_TEST_SUITE_P(
        FailureReasonToStringWorks, FailureReasonToStringTest,
        ValuesIn({
                FailureReasonToStringTestParam{AVERIFIED_DEX2OAT_TIMEOUT,
                                               "AVERIFIED_DEX2OAT_TIMEOUT"},
                FailureReasonToStringTestParam{AVERIFIED_DEX2OAT_COMPILATION_SETUP_FAILED,
                                               "AVERIFIED_DEX2OAT_COMPILATION_SETUP_FAILED"},
                FailureReasonToStringTestParam{AVERIFIED_DEX2OAT_DEX2OAT_FAILED,
                                               "AVERIFIED_DEX2OAT_DEX2OAT_FAILED"},
                FailureReasonToStringTestParam{AVERIFIED_DEX2OAT_FAILED_TO_ENABLE_FSVERITY,
                                               "AVERIFIED_DEX2OAT_FAILED_TO_ENABLE_FSVERITY"},
        }));

struct StatusToStringTestParam {
    AVerifiedDex2Oat_Status status;
    std::string expected_string;
};

class StatusToStringTest : public TestWithParam<StatusToStringTestParam> {};

TEST_P(StatusToStringTest, Works) {
    const auto& [status, expected_string] = GetParam();
    const char* result = AVerifiedDex2Oat_Status_toString(status);
    EXPECT_STREQ(result, expected_string.c_str());
}

INSTANTIATE_TEST_SUITE_P(
        StatusToStringWorks, StatusToStringTest,
        ValuesIn({
                StatusToStringTestParam{AVERIFIED_DEX2OAT_SUCCESS, "AVERIFIED_DEX2OAT_SUCCESS"},
                StatusToStringTestParam{AVERIFIED_DEX2OAT_ERROR_GENERAL,
                                        "AVERIFIED_DEX2OAT_ERROR_GENERAL"},
                StatusToStringTestParam{AVERIFIED_DEX2OAT_ERROR_TIMED_OUT,
                                        "AVERIFIED_DEX2OAT_ERROR_TIMED_OUT"},
                StatusToStringTestParam{AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE,
                                        "AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE"},
                StatusToStringTestParam{AVERIFIED_DEX2OAT_ERROR_CALLING_COMPOS,
                                        "AVERIFIED_DEX2OAT_ERROR_CALLING_COMPOS"},
                StatusToStringTestParam{AVERIFIED_DEX2OAT_BAD_ARGS, "AVERIFIED_DEX2OAT_BAD_ARGS"},
                StatusToStringTestParam{AVERIFIED_DEX2OAT_BAD_ARGS_FORMAT_STRING_NOT_UTF8,
                                        "AVERIFIED_DEX2OAT_BAD_ARGS_FORMAT_STRING_NOT_UTF8"},
                StatusToStringTestParam{AVERIFIED_DEX2OAT_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS,
                                        "AVERIFIED_DEX2OAT_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS"},
                StatusToStringTestParam{AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE,
                                        "AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE"},
                StatusToStringTestParam{AVERIFIED_DEX2OAT_CTX_MISSING_ARGS,
                                        "AVERIFIED_DEX2OAT_CTX_MISSING_ARGS"},
        }));

} // namespace compos_api