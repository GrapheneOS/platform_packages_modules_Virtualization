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
#include <aidl/com/android/microdroid/testservice/BnTestTenantService.h>
#include <aidl/com/android/microdroid/testservice/BnVmCallback.h>
#include <aidl/com/android/microdroid/testservice/IAppCallback.h>
#include <aidl/com/android/microdroid/testservice/ITestTenantService.h>
#include <android-base/chrono_utils.h>
#include <android-base/file.h>
#include <android-base/properties.h>
#include <android-base/result.h>
#include <android-base/scopeguard.h>
#include <android/binder_libbinder.h>
#include <android/log.h>
#include <binder/RpcSession.h>
#include <fcntl.h>
#include <fstab/fstab.h>
#include <fsverity_digests.pb.h>
#include <linux/vm_sockets.h>
#include <poll.h>
#include <selinux/selinux.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/capability.h>
#include <sys/inotify.h>
#include <sys/statvfs.h>
#include <sys/system_properties.h>

#include <chrono>
#include <filesystem>
#ifdef __MICRODROID_TEST_PAYLOAD_USES_LIBICU__
#include <unicode/uchar.h>
#endif
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
using android::base::WaitForProperty;
using android::fs_mgr::Fstab;
using android::fs_mgr::FstabEntry;
using android::fs_mgr::GetEntryForMountPoint;
using android::fs_mgr::ReadFstabFromFile;

using aidl::com::android::microdroid::testservice::BnTestService;
using aidl::com::android::microdroid::testservice::BnTestTenantService;
using aidl::com::android::microdroid::testservice::BnVmCallback;
using aidl::com::android::microdroid::testservice::IAppCallback;
using aidl::com::android::microdroid::testservice::ITestTenantService;
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
            if (errno == 0) {
                return {}; // the input was closed
            }
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
        Result<void> result;
        while ((result = run_echo_reverse_server(listening_fd)).ok()) {
        }
        __android_log_write(ANDROID_LOG_ERROR, TAG, result.error().message().c_str());
        // Make sure the VM exits so the test will fail solidly
        exit(1);
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

        ScopedAStatus getEncryptedStorageSize(int64_t* out) override {
            const char* path_c = AVmPayload_getEncryptedStoragePath();
            if (path_c == nullptr) {
                *out = 0;
                return ScopedAStatus::ok();
            }
            struct statvfs buffer;
            if (statvfs(path_c, &buffer) != 0) {
                std::string msg =
                        "statvfs " + std::string(path_c) + " failed :  " + std::strerror(errno);
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   msg.c_str());
            }
            *out = buffer.f_blocks * buffer.f_frsize;
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

        // Function to get the SELinux domain of the current process
        ScopedAStatus getselinuxdomain(std::string* out) {
            char* context = nullptr;
            int ret = getcon(&context);
            if (ret != 0) {
                return ScopedAStatus::fromServiceSpecificErrorWithMessage(0, "Failed to getCon");
            }
            *out = context;
            freecon(context);
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
                        ". Errno: " + std::to_string(errno);
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   msg.c_str());
            }
            return ScopedAStatus::ok();
        }

        ScopedAStatus readFromFile(const std::string& path, std::string* out) override {
            if (!android::base::ReadFileToString(path, out)) {
                std::string msg =
                        "Failed to read " + path + " to string. Errno: " + std::to_string(errno);
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

        ScopedAStatus insecurelyReadPayloadRpData(std::array<uint8_t, 32>* out) override {
            if (__builtin_available(android 36, *)) {
                int32_t ret = AVmPayload_readRollbackProtectedSecret(out->data(), 32);
                if (ret != 32) {
                    return ScopedAStatus::fromServiceSpecificError(ret);
                }
                return ScopedAStatus::ok();
            } else {
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   "not available before SDK 36");
            }
        }

        ScopedAStatus insecurelyWritePayloadRpData(
                const std::array<uint8_t, 32>& inputData) override {
            if (__builtin_available(android 36, *)) {
                int32_t ret = AVmPayload_writeRollbackProtectedSecret(inputData.data(), 32);
                if (ret != 32) {
                    return ScopedAStatus::fromServiceSpecificError(ret);
                }
                return ScopedAStatus::ok();
            } else {
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   "not available before SDK 36");
            }
        }

        ScopedAStatus isNewInstance(bool* is_new_instance_out) override {
            if (__builtin_available(android 36, *)) {
                *is_new_instance_out = AVmPayload_isNewInstance();
                return ScopedAStatus::ok();
            } else {
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   "not available before SDK 36");
            }
        }

        ScopedAStatus checkLibIcuIsAccessible() override {
#ifdef __MICRODROID_TEST_PAYLOAD_USES_LIBICU__
            static constexpr const char* kLibIcuPath = "/apex/com.android.i18n/lib64/libicu.so";
            if (access(kLibIcuPath, R_OK) == 0) {
                if (!u_hasBinaryProperty(U'❤' /* Emoji heart U+2764 */, UCHAR_EMOJI)) {
                    return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                       "libicu broken!");
                }
                return ScopedAStatus::ok();
            } else {
                std::string msg = "failed to access " + std::string(kLibIcuPath) + "(" +
                        std::to_string(errno) + ")";
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   msg.c_str());
            }
#else
            return ScopedAStatus::
                    fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                 "should be only used together with "
                                                 "MicrodroidTestNativeLibWithLibIcu.so payload");
#endif
        }

        ScopedAStatus requestEncryptedStoreSetup() override {
            const char* path_c = AVmPayload_getEncryptedStoragePath();
            if (access(path_c, F_OK) == 0) {
                std::string err_msg = std::string(path_c) + " already exist";
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   err_msg.c_str());
            }
            if (__system_property_set("microdroid_manager.encrypted_store.setup", "true") != 0) {
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   "Failed to set "
                                                                   "microdroid_manager.encrypted_"
                                                                   "store.setup sysprop");
            }

            if (!WaitForProperty("microdroid_manager.encrypted_store.status", "ready", 5s)) {
                return ScopedAStatus::
                        fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                     "encrypted store not available after 5s");
            }
            return ScopedAStatus::ok();
        }

        ScopedAStatus getHostname(std::string* out) override {
            char hostname[64];
            if (gethostname(hostname, 64) != 0) {
                std::string msg = "gethostname failed (" + std::to_string(errno) + ")";
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   msg.c_str());
            }
            *out = std::string(hostname);
            return ScopedAStatus::ok();
        }

        ScopedAStatus mountEncryptedAssets(const std::string& path,
                                           std::string* out_mount_point) override {
            if (__builtin_available(android 37, *)) {
                // Created by `xxd -i tests/encrypted_assets/test.key`.
                constexpr unsigned char kTestKey[64] = {0xfe, 0xed, 0xc0, 0xde, 0x54, 0xaf, 0x81,
                                                        0x86, 0xf3, 0x17, 0xb3, 0x9c, 0x17, 0x0c,
                                                        0xdc, 0xe2, 0xe5, 0xda, 0x0e, 0xca, 0x91,
                                                        0x19, 0x14, 0xdd, 0xaa, 0xd7, 0x26, 0x41,
                                                        0x02, 0x34, 0x53, 0xb7, 0x6d, 0x90, 0xa7,
                                                        0x7c, 0xfe, 0x4c, 0xfe, 0xfb, 0xa0, 0xb9,
                                                        0xd6, 0xb6, 0x4e, 0xd4, 0x45, 0xc0, 0x38,
                                                        0xce, 0x4d, 0xfc, 0xe1, 0xd5, 0x2d, 0xad,
                                                        0x53, 0xed, 0x24, 0x9c, 0x1f, 0xf1, 0x00,
                                                        0x94};
                constexpr int kSectorSize = 512;

                char mount_point[PATH_MAX] = {};
                int status =
                        AVmPayload_mountEncryptedAssets(path.c_str(), "erofs", "aes-xts-plain64",
                                                        kTestKey, sizeof(kTestKey), kSectorSize,
                                                        mount_point, sizeof(mount_point));
                if (status != 0) {
                    return ScopedAStatus::
                            fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                         "Failed to mount encrypted asset");
                }
                *out_mount_point = mount_point;
                return ScopedAStatus::ok();
            } else {
                return ScopedAStatus::fromExceptionCodeWithMessage(EX_SERVICE_SPECIFIC,
                                                                   "not available before SDK 36");
            }
        }

        ScopedAStatus quit() override { exit(0); }

        ScopedAStatus startUdsServerWithData(const std::string& data) override {
            class TestTenantService : public BnTestTenantService {
            private:
                std::string data;

            public:
                TestTenantService(const std::string& data) { this->data = data; }
                ScopedAStatus getData(std::string* out) override {
                    *out = data;
                    return ScopedAStatus::ok();
                }
            };

            auto testTenantService = ndk::SharedRefBase::make<TestTenantService>(data);
            auto callback = []([[maybe_unused]] void* param) { AVmPayload_notifyPayloadReady(); };

            std::thread udsServerThread([testTenantService, callback] {
                std::string descriptor = std::string(testTenantService->descriptor);
                AVmPayload_runUnixDomainRpcServer(descriptor.c_str(),
                                                  testTenantService->asBinder().get(), callback,
                                                  nullptr);
            });
            udsServerThread.detach();
            return ScopedAStatus::ok();
        }

        ScopedAStatus startUdsClientAndGetData(std::string* out) override {
            auto client_session_result = startUdsClient();
            if (!client_session_result.ok()) {
                return ScopedAStatus::fromServiceSpecificErrorWithMessage(-1,
                                                                          client_session_result
                                                                                  .error()
                                                                                  .message()
                                                                                  .c_str());
            }
            auto client_session = *client_session_result;
            auto platform_binder = client_session->getRootObject();
            std::shared_ptr<ITestTenantService> service = ITestTenantService::fromBinder(
                    ndk::SpAIBinder(AIBinder_fromPlatformBinder(platform_binder)));
            ScopedAStatus get_data_status = service->getData(out);
            if (!get_data_status.isOk()) {
                return get_data_status;
            }
            return ScopedAStatus::ok();
        }

    private:
        Result<android::sp<android::RpcSession>> startUdsClient() {
            const std::string socket_dir = "/dev/socket/microdroid_managed";
            std::string socket_path =
                    socket_dir + "/" + std::string(ITestTenantService::descriptor);

            constexpr std::chrono::seconds kUdsSocketTimeout = 5s;

            if (access(socket_path.c_str(), F_OK) != 0) {
                if (errno != ENOENT) {
                    return ErrnoError() << "failed to access UDS socket " + socket_path;
                }

                unique_fd inotify_fd(inotify_init1(IN_CLOEXEC));
                if (!inotify_fd.ok()) {
                    return ErrnoError() << "inotify_init1 failed";
                }

                int wd = inotify_add_watch(inotify_fd.get(), socket_dir.c_str(), IN_CREATE);
                if (wd < 0) {
                    return ErrnoError() << "inotify_add_watch failed for " << socket_dir;
                }
                auto remove_watch_guard =
                        make_scope_guard([&] { inotify_rm_watch(inotify_fd.get(), wd); });

                // Check again for race
                if (access(socket_path.c_str(), F_OK) != 0) {
                    if (errno != ENOENT) {
                        return ErrnoError() << "failed to access UDS socket " + socket_path;
                    }

                    pollfd pfd = {.fd = inotify_fd.get(), .events = POLLIN};
                    int poll_ret = poll(&pfd, 1, kUdsSocketTimeout.count() * 1000);

                    if (poll_ret <= 0) { // timeout or error
                        std::string msg = "UDS socket " + socket_path +
                                " did not become available within " +
                                std::to_string(kUdsSocketTimeout.count()) + " seconds.";
                        return Error() << msg;
                    }
                }
            }

            if (access(socket_path.c_str(), F_OK) != 0) {
                std::string msg = "UDS socket " + socket_path +
                        " did not become available within " +
                        std::to_string(kUdsSocketTimeout.count()) + " seconds.";
                return Error() << msg;
            }

            auto session = android::RpcSession::make();
            if (session->setupUnixDomainClient(socket_path.c_str()) != android::OK) {
                return ErrnoError() << "failed to setup Unix Domain client";
            }
            return session;
        }
    };
    auto testService = ndk::SharedRefBase::make<TestService>();

    auto callback = []([[maybe_unused]] void* param) { AVmPayload_notifyPayloadReady(); };
    int port;
#ifdef __USE_ALTERNATE_PORT__
    port = testService->ALTERNATE_PORT;
#else
    port = testService->PORT;
#endif

    AVmPayload_runVsockRpcServer(testService->asBinder().get(), port, callback, nullptr);

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

Result<void> verify_packages_mounted() {
    const char* path = "/mnt/tenant-apk";
    std::error_code ec;
    bool empty = std::filesystem::is_empty(path, ec);
    if (ec) {
        const std::string message = "Failed to check " + std::string(path) + ": " + ec.message();
        __android_log_write(ANDROID_LOG_INFO, TAG, message.c_str());
    }
    if (empty) {
        const std::string message = "No packages mounted in " + std::string(path);
        __android_log_write(ANDROID_LOG_INFO, TAG, message.c_str());
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
    report_test("tenant_packages_mounted", verify_packages_mounted());

    __system_property_set("debug.microdroid.app.run", "true");

    if (auto res = start_test_service(); res.ok()) {
        return 0;
    } else {
        __android_log_write(ANDROID_LOG_ERROR, TAG, res.error().message().c_str());
        return 1;
    }
}
