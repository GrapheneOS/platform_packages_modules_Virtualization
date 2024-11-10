/*
 * Copyright (C) 2021 The Android Open Source Project
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

#include <aidl/com/android/microdroid/testservice/BnTestService.h>
#include <aidl/com/android/microdroid/testservice/BnVmCallback.h>
#include <aidl/com/android/microdroid/testservice/IAppCallback.h>
#include <android-base/file.h>
#include <android-base/properties.h>
#include <android-base/result.h>
#include <android-base/scopeguard.h>
#include <android/log.h>
#include <fcntl.h>
#include <fstab/fstab.h>
#include <fsverity_digests.pb.h>
#include <linux/vm_sockets.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/capability.h>
#include <sys/system_properties.h>
#include <unistd.h>
#include <vm_main.h>
#include <vm_payload_restricted.h>

#include <cstdint>
#include <string>
#include <thread>

using android::base::borrowed_fd;
using android::base::ErrnoError;
using android::base::Error;
using android::base::make_scope_guard;
using android::base::Result;
using android::base::unique_fd;
using android::fs_mgr::Fstab;
using android::fs_mgr::FstabEntry;
using android::fs_mgr::GetEntryForMountPoint;
using android::fs_mgr::ReadFstabFromFile;

using aidl::com::android::microdroid::testservice::BnTestService;
using aidl::com::android::microdroid::testservice::BnVmCallback;
using aidl::com::android::microdroid::testservice::IAppCallback;
using ndk::ScopedAStatus;

extern void testlib_sub();

namespace {

constexpr char TAG[] = "testbinary";

template <typename T>
Result<T> report_test(std::string name, Result<T> result) {
    auto property = "debug.microdroid.test." + name;
    std::stringstream outcome;
    if (result.ok()) {
        outcome << "PASS";
    } else {
        outcome << "FAIL: " << result.error();
        // Log the error in case the property is truncated.
        std::string message = name + ": " + outcome.str();
        __android_log_write(ANDROID_LOG_WARN, TAG, message.c_str());
    }
    __system_property_set(property.c_str(), outcome.str().c_str());
    return result;
}

Result<void> run_echo_reverse_server(borrowed_fd listening_fd) {
    struct sockaddr_vm client_sa = {};
    socklen_t client_sa_len = sizeof(client_sa);
    unique_fd connect_fd{accept4(listening_fd.get(), (struct sockaddr*)&client_sa, &client_sa_len,
                                 SOCK_CLOEXEC)};
    if (!connect_fd.ok()) {
        return ErrnoError() << "Failed to accept vsock connection";
    }

    unique_fd input_fd{fcntl(connect_fd, F_DUPFD_CLOEXEC, 0)};
    if (!input_fd.ok()) {
        return ErrnoError() << "Failed to dup";
    }
    FILE* input = fdopen(input_fd.release(), "r");
    if (!input) {
        return ErrnoError() << "Failed to fdopen";
    }

    // Run forever, reverse one line at a time.
    while (true) {
        char* line = nullptr;
        size_t size = 0;
        if (getline(&line, &size, input) < 0) {
            return ErrnoError() << "Failed to read";
        }

        std::string_view original = line;
        if (!original.empty() && original.back() == '\n') {
            original = original.substr(0, original.size() - 1);
        }

        std::string reversed(original.rbegin(), original.rend());
        reversed += "\n";

        if (write(connect_fd, reversed.data(), reversed.size()) < 0) {
            return ErrnoError() << "Failed to write";
        }
    }
}

Result<void> start_echo_reverse_server() {
    unique_fd server_fd{TEMP_FAILURE_RETRY(socket(AF_VSOCK, SOCK_STREAM | SOCK_CLOEXEC, 0))};
    if (!server_fd.ok()) {
        return ErrnoError() << "Failed to create vsock socket";
    }
    struct sockaddr_vm server_sa = (struct sockaddr_vm){
            .svm_family = AF_VSOCK,
            .svm_port = static_cast<uint32_t>(BnTestService::ECHO_REVERSE_PORT),
            .svm_cid = VMADDR_CID_ANY,
    };
    int ret = TEMP_FAILURE_RETRY(bind(server_fd, (struct sockaddr*)&server_sa, sizeof(server_sa)));
    if (ret < 0) {
        return ErrnoError() << "Failed to bind vsock socket";
    }
    ret = TEMP_FAILURE_RETRY(listen(server_fd, /*backlog=*/1));
    if (ret < 0) {
        return ErrnoError() << "Failed to listen";
    }

    std::thread accept_thread{[listening_fd = std::move(server_fd)] {
        auto result = run_echo_reverse_server(listening_fd);
        if (!result.ok()) {
            __android_log_write(ANDROID_LOG_ERROR, TAG, result.error().message().c_str());
            // Make sure the VM exits so the test will fail solidly
            exit(1);
        }
    }};
    accept_thread.detach();

    return {};
}

Result<void> start_test_service() {
    class VmCallbackImpl : public BnVmCallback {
    private:
        std::shared_ptr<IAppCallback> mAppCallback;

    public:
        explicit VmCallbackImpl(const std::shared_ptr<IAppCallback>& appCallback)
              : mAppCallback(appCallback) {}

        ScopedAStatus echoMessage(const std::string& message) override {
            std::thread callback_thread{[=, appCallback = mAppCallback] {
                appCallback->onEchoRequestReceived("Received: " + message);
            }};
            callback_thread.detach();
            return ScopedAStatus::ok();
        }
    };

    class TestService : public BnTestService {
    public:
        ScopedAStatus addInteger(int32_t a, int32_t b, int32_t* out) override {
            *out = a + b;
            return ScopedAStatus::ok();
        }

        ScopedAStatus readProperty(const std::string& prop, std::string* out) override {
            *out = android::base::GetProperty(prop, "");
            if (out->empty()) {
                std::string msg = "cannot find property " + prop;
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   msg.c_str());
            }

            return ScopedAStatus::ok();
        }

        ScopedAStatus insecurelyExposeVmInstanceSecret(std::vector<uint8_t>* out) override {
            const uint8_t identifier[] = {1, 2, 3, 4};
            out->resize(32);
            AVmPayload_getVmInstanceSecret(identifier, sizeof(identifier), out->data(),
                                           out->size());
            return ScopedAStatus::ok();
        }

        ScopedAStatus insecurelyExposeAttestationCdi(std::vector<uint8_t>* out) override {
            size_t cdi_size = AVmPayload_getDiceAttestationCdi(nullptr, 0);
            out->resize(cdi_size);
            AVmPayload_getDiceAttestationCdi(out->data(), out->size());
            return ScopedAStatus::ok();
        }

        ScopedAStatus getBcc(std::vector<uint8_t>* out) override {
            size_t bcc_size = AVmPayload_getDiceAttestationChain(nullptr, 0);
            out->resize(bcc_size);
            AVmPayload_getDiceAttestationChain(out->data(), out->size());
            return ScopedAStatus::ok();
        }

        ScopedAStatus getApkContentsPath(std::string* out) override {
            const char* path_c = AVmPayload_getApkContentsPath();
            if (path_c == nullptr) {
                return ScopedAStatus::
                        fromServiceSpecificErrorWithMessage(0, "Failed to get APK contents path");
            }
            *out = path_c;
            return ScopedAStatus::ok();
        }

        ScopedAStatus getEncryptedStoragePath(std::string* out) override {
            const char* path_c = AVmPayload_getEncryptedStoragePath();
            if (path_c == nullptr) {
                out->clear();
            } else {
                *out = path_c;
            }
            return ScopedAStatus::ok();
        }

        ScopedAStatus getEffectiveCapabilities(std::vector<std::string>* out) override {
            if (out == nullptr) {
                return ScopedAStatus::ok();
            }
            cap_t cap = cap_get_proc();
            auto guard = make_scope_guard([&cap]() { cap_free(cap); });
            for (cap_value_t cap_id = 0; cap_id < CAP_LAST_CAP + 1; cap_id++) {
                cap_flag_value_t value;
                if (cap_get_flag(cap, cap_id, CAP_EFFECTIVE, &value) != 0) {
                    return ScopedAStatus::
                            fromServiceSpecificErrorWithMessage(0, "cap_get_flag failed");
                }
                if (value == CAP_SET) {
                    // Ideally we would just send back the cap_ids, but I wasn't able to find java
                    // APIs for linux capabilities, hence we transform to the human readable name
                    // here.
                    char* name = cap_to_name(cap_id);
                    out->push_back(std::string(name) + "(" + std::to_string(cap_id) + ")");
                }
            }
            return ScopedAStatus::ok();
        }

        ScopedAStatus getUid(int* out) override {
            *out = getuid();
            return ScopedAStatus::ok();
        }

        ScopedAStatus runEchoReverseServer() override {
            auto result = start_echo_reverse_server();
            if (result.ok()) {
                return ScopedAStatus::ok();
            } else {
                std::string message = result.error().message();
                return ScopedAStatus::fromServiceSpecificErrorWithMessage(-1, message.c_str());
            }
        }

        ScopedAStatus writeToFile(const std::string& content, const std::string& path) override {
            if (!android::base::WriteStringToFile(content, path)) {
                std::string msg = "Failed to write " + content + " to file " + path +
                        ". Errono: " + std::to_string(errno);
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   msg.c_str());
            }
            return ScopedAStatus::ok();
        }

        ScopedAStatus readFromFile(const std::string& path, std::string* out) override {
            if (!android::base::ReadFileToString(path, out)) {
                std::string msg =
                        "Failed to read " + path + " to string. Errono: " + std::to_string(errno);
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   msg.c_str());
            }
            return ScopedAStatus::ok();
        }

        ScopedAStatus getFilePermissions(const std::string& path, int32_t* out) override {
            struct stat sb;
            if (stat(path.c_str(), &sb) != -1) {
                *out = sb.st_mode;
            } else {
                std::string msg = "stat " + path + " failed :  " + std::strerror(errno);
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   msg.c_str());
            }
            return ScopedAStatus::ok();
        }

        ScopedAStatus getMountFlags(const std::string& mount_point, int32_t* out) override {
            Fstab fstab;
            if (!ReadFstabFromFile("/proc/mounts", &fstab)) {
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   "Failed to read /proc/mounts");
            }
            FstabEntry* entry = GetEntryForMountPoint(&fstab, mount_point);
            if (entry == nullptr) {
                std::string msg = mount_point + " not found in /proc/mounts";
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   msg.c_str());
            }
            *out = entry->flags;
            return ScopedAStatus::ok();
        }

        ScopedAStatus getPageSize(int32_t* out) override {
            *out = getpagesize();
            return ScopedAStatus::ok();
        }

        ScopedAStatus requestCallback(const std::shared_ptr<IAppCallback>& appCallback) {
            auto vmCallback = ndk::SharedRefBase::make<VmCallbackImpl>(appCallback);
            std::thread callback_thread{[=] { appCallback->setVmCallback(vmCallback); }};
            callback_thread.detach();
            return ScopedAStatus::ok();
        }

        ScopedAStatus readLineFromConsole(std::string* out) {
            FILE* f = fopen("/dev/console", "r");
            if (f == nullptr) {
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   "failed to open /dev/console");
            }
            char* line = nullptr;
            size_t len = 0;
            ssize_t nread = getline(&line, &len, f);

            if (nread == -1) {
                free(line);
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   "failed to read /dev/console");
            }
            out->append(line, nread);
            free(line);
            return ScopedAStatus::ok();
        }

        ScopedAStatus quit() override { exit(0); }
    };
    auto testService = ndk::SharedRefBase::make<TestService>();

    auto callback = []([[maybe_unused]] void* param) { AVmPayload_notifyPayloadReady(); };
    AVmPayload_runVsockRpcServer(testService->asBinder().get(), testService->PORT, callback,
                                 nullptr);

    return {};
}

Result<void> verify_build_manifest() {
    const char* path = "/mnt/extra-apk/0/assets/build_manifest.pb";

    std::string str;
    if (!android::base::ReadFileToString(path, &str)) {
        return ErrnoError() << "failed to read build_manifest.pb";
    }

    if (!android::security::fsverity::FSVerityDigests().ParseFromString(str)) {
        return Error() << "invalid build_manifest.pb";
    }

    return {};
}

Result<void> verify_vm_share() {
    const char* path = "/mnt/extra-apk/0/assets/vmshareapp.txt";

    std::string str;
    if (!android::base::ReadFileToString(path, &str)) {
        return ErrnoError() << "failed to read vmshareapp.txt";
    }

    return {};
}

} // Anonymous namespace

extern "C" int AVmPayload_main() {
    __android_log_write(ANDROID_LOG_INFO, TAG, "Hello Microdroid");

    // Make sure we can call into other shared libraries.
    testlib_sub();

    // Report various things that aren't always fatal - these are checked in MicrodroidTests as
    // appropriate.
    report_test("extra_apk_build_manifest", verify_build_manifest());
    report_test("extra_apk_vm_share", verify_vm_share());

    __system_property_set("debug.microdroid.app.run", "true");

    if (auto res = start_test_service(); res.ok()) {
        return 0;
    } else {
        __android_log_write(ANDROID_LOG_ERROR, TAG, res.error().message().c_str());
        return 1;
    }
}
