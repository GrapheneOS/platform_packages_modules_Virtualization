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
#include <aidl/android/system/virtualizationservice/IVirtualizationService.h>
#include <android-base/file.h>
#include <android-base/result.h>
#include <android-base/unique_fd.h>
#include <fuzzbinder/libbinder_ndk_driver.h>
#include <fuzzer/FuzzedDataProvider.h>
#include <unistd.h>

#include <binder_rpc_unstable.hpp>
#include <cstdlib>
#include <iostream>

using aidl::android::system::virtualizationservice::IVirtualizationService;
using android::fuzzService;
using android::base::ErrnoError;
using android::base::Error;
using android::base::Pipe;
using android::base::Result;
using android::base::Socketpair;
using android::base::unique_fd;
using ndk::SpAIBinder;

static constexpr const char VIRTMGR_PATH[] = "/apex/com.android.virt/bin/virtmgr";
static constexpr size_t VIRTMGR_THREADS = 2;

Result<unique_fd> get_service_fd() {
    unique_fd server_fd, client_fd;
    if (!Socketpair(SOCK_STREAM, &server_fd, &client_fd)) {
        return ErrnoError() << "Failed to create socketpair";
    }

    unique_fd wait_fd, ready_fd;
    if (!Pipe(&wait_fd, &ready_fd, 0)) {
        return ErrnoError() << "Failed to create pipe";
    }

    if (int pid = fork(); pid == 0) {
        client_fd.reset();
        wait_fd.reset();

        auto server_fd_str = std::to_string(server_fd.get());
        auto ready_fd_str = std::to_string(ready_fd.get());

        if (execl(VIRTMGR_PATH, VIRTMGR_PATH, "--rpc-server-fd", server_fd_str.c_str(),
                  "--ready-fd", ready_fd_str.c_str(), nullptr) == -1) {
            return ErrnoError() << "Failed to execute virtmgr";
        }
    } else if (pid < 0) {
        return ErrnoError() << "Failed to fork";
    }

    server_fd.reset();
    ready_fd.reset();

    char buf;
    if (read(wait_fd.get(), &buf, sizeof(buf)) < 0) {
        return ErrnoError() << "Failed to wait for VirtualizationService to be ready";
    }

    return client_fd;
}

Result<std::shared_ptr<IVirtualizationService>> connect_service(int fd) {
    std::unique_ptr<ARpcSession, decltype(&ARpcSession_free)> session(ARpcSession_new(),
                                                                      &ARpcSession_free);
    ARpcSession_setFileDescriptorTransportMode(session.get(),
                                               ARpcSession_FileDescriptorTransportMode::Unix);
    ARpcSession_setMaxIncomingThreads(session.get(), VIRTMGR_THREADS);
    ARpcSession_setMaxOutgoingConnections(session.get(), VIRTMGR_THREADS);
    AIBinder* binder = ARpcSession_setupUnixDomainBootstrapClient(session.get(), fd);
    if (binder == nullptr) {
        return Error() << "Failed to connect to VirtualizationService";
    }
    return IVirtualizationService::fromBinder(SpAIBinder{binder});
}

Result<void> inner_fuzz(const uint8_t* data, size_t size) {
    unique_fd fd = OR_RETURN(get_service_fd());
    std::shared_ptr<IVirtualizationService> service = OR_RETURN(connect_service(fd.get()));
    fuzzService(service->asBinder().get(), FuzzedDataProvider(data, size));

    return {};
}

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
    if (auto ret = inner_fuzz(data, size); !ret.ok()) {
        std::cerr << "connecting to service failed: " << ret.error() << std::endl;
        abort();
    }
    return 0;
}
