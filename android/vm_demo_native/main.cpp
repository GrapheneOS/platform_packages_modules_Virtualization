/*
 * Copyright 2023 The Android Open Source Project
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
#include <aidl/android/system/virtualizationcommon/DeathReason.h>
#include <aidl/android/system/virtualizationcommon/ErrorCode.h>
#include <aidl/android/system/virtualizationservice/BnVirtualMachineCallback.h>
#include <aidl/android/system/virtualizationservice/IVirtualMachine.h>
#include <aidl/android/system/virtualizationservice/IVirtualMachineCallback.h>
#include <aidl/android/system/virtualizationservice/IVirtualizationService.h>
#include <aidl/android/system/virtualizationservice/VirtualMachineConfig.h>
#include <aidl/android/system/virtualizationservice/VirtualMachineState.h>
#include <aidl/com/android/microdroid/testservice/ITestService.h>
#include <android-base/errors.h>
#include <android-base/file.h>
#include <android-base/result.h>
#include <android-base/unique_fd.h>
#include <stdio.h>
#include <unistd.h>

#include <binder_rpc_unstable.hpp>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <memory>
#include <mutex>
#include <thread>

using namespace std::chrono_literals;

using android::base::ErrnoError;
using android::base::Error;
using android::base::Pipe;
using android::base::Result;
using android::base::Socketpair;
using android::base::unique_fd;

using ndk::ScopedAStatus;
using ndk::ScopedFileDescriptor;
using ndk::SharedRefBase;
using ndk::SpAIBinder;

using aidl::android::system::virtualizationcommon::DeathReason;
using aidl::android::system::virtualizationcommon::ErrorCode;
using aidl::android::system::virtualizationservice::BnVirtualMachineCallback;
using aidl::android::system::virtualizationservice::IVirtualizationService;
using aidl::android::system::virtualizationservice::IVirtualMachine;
using aidl::android::system::virtualizationservice::PartitionType;
using aidl::android::system::virtualizationservice::toString;
using aidl::android::system::virtualizationservice::VirtualMachineAppConfig;
using aidl::android::system::virtualizationservice::VirtualMachineConfig;
using aidl::android::system::virtualizationservice::VirtualMachinePayloadConfig;
using aidl::android::system::virtualizationservice::VirtualMachineState;

using aidl::com::android::microdroid::testservice::ITestService;

// This program demonstrates a way to run a VM and do something in the VM using AVF in the C++
// language. Instructions for building and running this demo can be found in `README.md` in this
// directory.

//--------------------------------------------------------------------------------------------------
// Step 1: connect to IVirtualizationService
//--------------------------------------------------------------------------------------------------
static constexpr const char VIRTMGR_PATH[] = "/apex/com.android.virt/bin/virtmgr";
static constexpr size_t VIRTMGR_THREADS = 2;

// Start IVirtualizationService instance and get FD for the unix domain socket that is connected to
// the service. The returned FD should be kept open until the service is no longer needed.
Result<unique_fd> get_service_fd() {
    unique_fd server_fd, client_fd;
    if (!Socketpair(SOCK_STREAM, &server_fd, &client_fd)) {
        return ErrnoError() << "Failed to create socketpair";
    }

    unique_fd wait_fd, ready_fd;
    if (!Pipe(&wait_fd, &ready_fd, 0)) {
        return ErrnoError() << "Failed to create pipe";
    }

    if (fork() == 0) {
        client_fd.reset();
        wait_fd.reset();

        auto server_fd_str = std::to_string(server_fd.get());
        auto ready_fd_str = std::to_string(ready_fd.get());

        if (execl(VIRTMGR_PATH, VIRTMGR_PATH, "--rpc-server-fd", server_fd_str.c_str(),
                  "--ready-fd", ready_fd_str.c_str(), nullptr) == -1) {
            return ErrnoError() << "Failed to execute virtmgr";
        }
    }

    server_fd.reset();
    ready_fd.reset();

    char buf;
    if (read(wait_fd.get(), &buf, sizeof(buf)) < 0) {
        return ErrnoError() << "Failed to wait for VirtualizationService to be ready";
    }

    return client_fd;
}

// Establish a binder communication channel over the unix domain socket and returns the remote
// IVirtualizationService.
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

//--------------------------------------------------------------------------------------------------
// Step 2: construct VirtualMachineAppConfig
//--------------------------------------------------------------------------------------------------

// Utility function for opening a file at a given path and wrap the resulting FD in
// ScopedFileDescriptor so that it can be passed to the service.
Result<ScopedFileDescriptor> open_file(const std::string& path, int flags) {
    int fd = open(path.c_str(), flags, S_IWUSR);
    if (fd == -1) {
        return ErrnoError() << "Failed to open " << path;
    }
    return ScopedFileDescriptor(fd);
}

// Create or update idsig file for the given APK file. The idsig is essentially a hashtree of the
// APK file's content
Result<ScopedFileDescriptor> create_or_update_idsig_file(IVirtualizationService& service,
                                                         const std::string& work_dir,
                                                         ScopedFileDescriptor& main_apk) {
    std::string path = work_dir + "/apk.idsig";
    ScopedFileDescriptor idsig = OR_RETURN(open_file(path, O_CREAT | O_RDWR));
    ScopedAStatus ret = service.createOrUpdateIdsigFile(main_apk, idsig);
    if (!ret.isOk()) {
        return Error() << "Failed to create or update idsig file: " << path;
    }
    return idsig;
}

// Get or create the instance disk image file, if it doesn't exist. The VM will fill this disk with
// its own identity information in an encrypted form.
Result<ScopedFileDescriptor> create_instance_image_file_if_needed(IVirtualizationService& service,
                                                                  const std::string& work_dir) {
    std::string path = work_dir + "/instance.img";

    // If instance.img already exists, use it.
    if (access(path.c_str(), F_OK) == 0) {
        return open_file(path, O_RDWR);
    }

    // If not, create a new one.
    ScopedFileDescriptor instance = OR_RETURN(open_file(path, O_CREAT | O_RDWR));
    long size = 10 * 1024 * 1024; // 10MB, but could be smaller.
    ScopedAStatus ret =
            service.initializeWritablePartition(instance, size, PartitionType::ANDROID_VM_INSTANCE);
    if (!ret.isOk()) {
        return Error() << "Failed to create instance disk image: " << path;
    }
    return instance;
}

// Construct VirtualMachineAppConfig for a Microdroid-based VM named `vm_name` that executes a
// shared library named `paylaod_binary_name` in the apk `main_apk_path`.
Result<VirtualMachineAppConfig> create_vm_config(
        IVirtualizationService& service, const std::string& work_dir, const std::string& vm_name,
        const std::string& main_apk_path, const std::string& payload_binary_name, bool debuggable,
        bool protected_vm, int32_t memory_mib) {
    ScopedFileDescriptor main_apk = OR_RETURN(open_file(main_apk_path, O_RDONLY));
    ScopedFileDescriptor idsig =
            OR_RETURN(create_or_update_idsig_file(service, work_dir, main_apk));
    ScopedFileDescriptor instance =
            OR_RETURN(create_instance_image_file_if_needed(service, work_dir));

    // There are two ways to specify the payload. The simpler way is by specifying the name of the
    // payload binary as shown below. The other way (which is allowed only to system-level VMs) is
    // by passing the path to the JSON file in the main APK which has detailed specification about
    // what to load in Microdroid. See packages/modules/Virtualization/compos/apk/assets/*.json as
    // examples.
    VirtualMachinePayloadConfig payload;
    payload.payloadBinaryName = payload_binary_name;

    VirtualMachineAppConfig app_config;
    app_config.name = vm_name;
    app_config.apk = std::move(main_apk);
    app_config.idsig = std::move(idsig);
    app_config.instanceImage = std::move(instance);
    app_config.payload = std::move(payload);
    if (debuggable) {
        app_config.debugLevel = VirtualMachineAppConfig::DebugLevel::FULL;
    }
    app_config.protectedVm = protected_vm;
    app_config.memoryMib = memory_mib;

    return app_config;
}

//--------------------------------------------------------------------------------------------------
// Step 3: create a VM and start it
//--------------------------------------------------------------------------------------------------

// Create a virtual machine with the config, but doesn't start it yet.
Result<std::shared_ptr<IVirtualMachine>> create_virtual_machine(
        IVirtualizationService& service, VirtualMachineAppConfig& app_config) {
    std::shared_ptr<IVirtualMachine> vm;

    VirtualMachineConfig config = std::move(app_config);
    ScopedFileDescriptor console_out_fd(fcntl(fileno(stdout), F_DUPFD_CLOEXEC));
    ScopedFileDescriptor console_in_fd(fcntl(fileno(stdin), F_DUPFD_CLOEXEC));
    ScopedFileDescriptor log_fd(fcntl(fileno(stdout), F_DUPFD_CLOEXEC));

    ScopedAStatus ret = service.createVm(config, console_out_fd, console_in_fd, log_fd, &vm);
    if (!ret.isOk()) {
        return Error() << "Failed to create VM";
    }
    return vm;
}

// When a VM lifecycle changes, a corresponding method in this class is called. This also provides
// methods for blocking the current thread until the VM reaches a specific state.
class Callback : public BnVirtualMachineCallback {
public:
    Callback(const std::shared_ptr<IVirtualMachine>& vm) : mVm(vm) {}

    ScopedAStatus onPayloadStarted(int32_t) {
        std::unique_lock lock(mMutex);
        mCv.notify_all();
        return ScopedAStatus::ok();
    }

    ScopedAStatus onPayloadReady(int32_t) {
        std::unique_lock lock(mMutex);
        mCv.notify_all();
        return ScopedAStatus::ok();
    }

    ScopedAStatus onPayloadFinished(int32_t, int32_t) {
        std::unique_lock lock(mMutex);
        mCv.notify_all();
        return ScopedAStatus::ok();
    }

    ScopedAStatus onError(int32_t, ErrorCode, const std::string&) {
        std::unique_lock lock(mMutex);
        mCv.notify_all();
        return ScopedAStatus::ok();
    }

    ScopedAStatus onDied(int32_t, DeathReason) {
        std::unique_lock lock(mMutex);
        mCv.notify_all();
        return ScopedAStatus::ok();
    }

    Result<void> wait_for_state(VirtualMachineState state) {
        std::unique_lock lock(mMutex);
        mCv.wait_for(lock, 5s, [this, &state] {
            auto cur_state = get_vm_state();
            return cur_state.ok() && *cur_state == state;
        });
        auto cur_state = get_vm_state();
        if (cur_state.ok()) {
            if (*cur_state == state) {
                return {};
            } else {
                return Error() << "Timeout waiting for state becomes " << toString(state);
            }
        }
        return cur_state.error();
    }

private:
    std::shared_ptr<IVirtualMachine> mVm;
    std::condition_variable mCv;
    std::mutex mMutex;

    Result<VirtualMachineState> get_vm_state() {
        VirtualMachineState state;
        ScopedAStatus ret = mVm->getState(&state);
        if (!ret.isOk()) {
            return Error() << "Failed to get state of virtual machine";
        }
        return state;
    }
};

// Start (i.e. boot) the virtual machine and return Callback monitoring the lifecycle event of the
// VM.
Result<std::shared_ptr<Callback>> start_virtual_machine(std::shared_ptr<IVirtualMachine> vm) {
    std::shared_ptr<Callback> cb = SharedRefBase::make<Callback>(vm);
    ScopedAStatus ret = vm->registerCallback(cb);
    if (!ret.isOk()) {
        return Error() << "Failed to register callback to virtual machine";
    }
    ret = vm->start();
    if (!ret.isOk()) {
        return Error() << "Failed to start virtual machine";
    }
    return cb;
}

//--------------------------------------------------------------------------------------------------
// Step 4: connect to the payload and communicate with it over binder/vsock
//--------------------------------------------------------------------------------------------------

// Connect to the binder service running in the payload.
Result<std::shared_ptr<ITestService>> connect_to_vm_payload(std::shared_ptr<IVirtualMachine> vm) {
    std::unique_ptr<ARpcSession, decltype(&ARpcSession_free)> session(ARpcSession_new(),
                                                                      &ARpcSession_free);
    ARpcSession_setMaxIncomingThreads(session.get(), 1);

    AIBinder* binder = ARpcSession_setupPreconnectedClient(
            session.get(),
            [](void* param) {
                std::shared_ptr<IVirtualMachine> vm =
                        *static_cast<std::shared_ptr<IVirtualMachine>*>(param);
                ScopedFileDescriptor sock_fd;
                ScopedAStatus ret = vm->connectVsock(ITestService::PORT, &sock_fd);
                if (!ret.isOk()) {
                    return -1;
                }
                return sock_fd.release();
            },
            &vm);
    if (binder == nullptr) {
        return Error() << "Failed to connect to vm payload";
    }
    return ITestService::fromBinder(SpAIBinder{binder});
}

// Do something with the service in the VM
Result<void> do_something(ITestService& payload) {
    int32_t result;
    ScopedAStatus ret = payload.addInteger(10, 20, &result);
    if (!ret.isOk()) {
        return Error() << "Failed to call addInteger";
    }
    std::cout << "The answer from VM is " << result << std::endl;
    return {};
}

// This is the main routine that follows the steps in order
Result<void> inner_main() {
    TemporaryDir work_dir;
    std::string work_dir_path(work_dir.path);

    // Step 1: connect to the virtualizationservice
    unique_fd fd = OR_RETURN(get_service_fd());
    std::shared_ptr<IVirtualizationService> service = OR_RETURN(connect_service(fd.get()));

    // Step 2: create vm config
    VirtualMachineAppConfig app_config = OR_RETURN(
            create_vm_config(*service, work_dir_path, "my_vm",
                             "/data/local/tmp/MicrodroidTestApp.apk", "MicrodroidTestNativeLib.so",
                             /* debuggable = */ true, // should be false for production VMs
                             /* protected_vm = */ true, 150));

    // Step 3: start vm
    std::shared_ptr<IVirtualMachine> vm = OR_RETURN(create_virtual_machine(*service, app_config));
    std::shared_ptr<Callback> cb = OR_RETURN(start_virtual_machine(vm));
    OR_RETURN(cb->wait_for_state(VirtualMachineState::READY));

    // Step 4: do something in the vm
    std::shared_ptr<ITestService> payload = OR_RETURN(connect_to_vm_payload(vm));
    OR_RETURN(do_something(*payload));

    // Step 5: let VM quit by itself, and wait for the graceful shutdown
    ScopedAStatus ret = payload->quit();
    if (!ret.isOk()) {
        return Error() << "Failed to command quit to the VM";
    }
    OR_RETURN(cb->wait_for_state(VirtualMachineState::DEAD));

    return {};
}

int main() {
    if (auto ret = inner_main(); !ret.ok()) {
        std::cerr << ret.error() << std::endl;
        return EXIT_FAILURE;
    }
    std::cout << "Done" << std::endl;
    return EXIT_SUCCESS;
}
