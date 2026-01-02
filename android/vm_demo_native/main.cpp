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
#include <android/content/pm/IPackageManagerNative.h>
#include <android/content/pm/PackageInfoNative.h>
#include <androidfw/AssetsProvider.h>
#include <binder/IServiceManager.h>
#include <json/json.h>
#include <stdio.h>
#include <unistd.h>

#include <binder_rpc_unstable.hpp>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <fstream>
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
using aidl::android::system::virtualizationcommon::IGuestAgent;
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

    pid_t pid = fork();
    if (pid == 0) {
        client_fd.reset();
        wait_fd.reset();

        auto server_fd_str = std::to_string(server_fd.get());
        auto ready_fd_str = std::to_string(ready_fd.get());

        if (execl(VIRTMGR_PATH, VIRTMGR_PATH, "--rpc-server-fd", server_fd_str.c_str(),
                  "--ready-fd", ready_fd_str.c_str(), nullptr) == -1) {
            return ErrnoError() << "Failed to execute virtmgr";
        }
    }

    // If this is executed on a terminal, let virtmgr be the foreground process group so that
    // it is not stopped.
    bool is_tty = isatty(0);
    if (is_tty) {
        tcsetpgrp(0, pid);
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

// Get or create the encryptedstore disk image file, if it doesn't exist.
Result<ScopedFileDescriptor> create_encryptedstore_image_file_if_needed(
        IVirtualizationService& service, const std::string& work_dir) {
    std::string path = work_dir + "/encryptedstore.img";

    // If encryptedstore.img already exists, use it.
    if (access(path.c_str(), F_OK) == 0) {
        return open_file(path, O_RDWR);
    }

    // If not, create a new one.
    ScopedFileDescriptor encryptedstore = OR_RETURN(open_file(path, O_CREAT | O_RDWR));
    long size = 10 * 1024 * 1024; // 10MB
    ScopedAStatus ret = service.initializeWritablePartition(encryptedstore, size,
                                                          PartitionType::ENCRYPTEDSTORE);
    if (!ret.isOk()) {
        return Error() << "Failed to create encryptedstore disk image: " << path;
    }
    return encryptedstore;
}

// This looks up instance-id in local file if it exists, otherwise requests virtualizationservice
// to allocate it one & then persists it in the instance-id file. VM uses this instance-id for
// Secret Management.
Result<void> get_or_allocate_instance_id(IVirtualizationService& service,
                                         const std::string& work_dir,
                                         std::array<uint8_t, 64>* instance_id) {
    std::string path = work_dir + "/instance_id";
    bool instance_id_exists;
    if (access(path.c_str(), F_OK) == 0) {
        instance_id_exists = true;
    } else {
        instance_id_exists = false;
    }
    unique_fd fd(open(path.c_str(), O_CREAT | O_RDWR | O_CLOEXEC, S_IRUSR | S_IWUSR));
    if (fd.get() == -1) {
        return ErrnoError() << "opening " << path << " failed";
    }

    if (instance_id_exists) {
        // If instance_id_file already exists, read from it!
        int n = read(fd, instance_id, instance_id->size());
        if (n < 0) {
            return ErrnoError() << "reading " << path << " failed";
        } else if (n != instance_id->size()) {
            return Error() << "Incomplete read of " << path;
        }
    } else {
        // If the instance-id does not exist, request for allocation & persist in the file
        ScopedAStatus ret = service.allocateInstanceId(instance_id);
        if (!ret.isOk()) {
            return Error() << "Failed to allocate Instance Id: ";
        }
        int n = write(fd, instance_id, instance_id->size());
        if (n < 0) {
            return ErrnoError() << "Writing " << path << " failed";
        } else if (n != instance_id->size()) {
            return Error() << "Incomplete write of " << path;
        }
    }
    return {};
}

// Get the package path using IPackageManagerNative interface.
Result<std::string> get_apk_path_from_package_name(const std::string& package_name) {
    android::sp<android::IBinder> binder =
            android::defaultServiceManager()->checkService(android::String16("package_native"));
    if (binder == nullptr) {
        return Error() << "Failed to get package_native service";
    }

    android::sp<android::content::pm::IPackageManagerNative> pm =
            android::interface_cast<android::content::pm::IPackageManagerNative>(binder);
    if (pm == nullptr) {
        return Error() << "Failed to get IPackageManagerNative";
    }

    int32_t user_id = 0;
    std::optional<android::content::pm::PackageInfoNative> package_info;
    android::binder::Status status =
            pm->getPackageInfoWithSigningInfo(android::String16(package_name.c_str()), user_id,
                                              &package_info);

    if (!status.isOk()) {
        return Error() << "Failed to get package info for " << package_name << ": "
                       << status.toString8().c_str();
    }

    if (!package_info || !package_info->sourceDir) {
        return Error() << "Package " << package_name << " not found or sourceDir is null";
    }

    return std::string(android::String8(*package_info->sourceDir).c_str());
}

// Parses the JSON config file from the main APK, finds any tenant APKs,
// prepares them, and adds them to the VirtualMachineAppConfig.
Result<void> add_tenant_apks_from_config(IVirtualizationService& service,
                                         const std::string& main_apk_path,
                                         const std::string& config_path_in_apk,
                                         const std::string& work_dir,
                                         VirtualMachineAppConfig* app_config) {
    auto assets_provider = android::ZipAssetsProvider::Create(main_apk_path, 0);
    if (!assets_provider) {
        return Error() << "Failed to create AssetsProvider for " << main_apk_path;
    }

    auto asset =
            assets_provider->Open(config_path_in_apk, android::Asset::AccessMode::ACCESS_BUFFER);
    if (!asset) {
        return Error() << "Failed to open " << config_path_in_apk << " in APK " << main_apk_path;
    }

    std::string json_content(static_cast<const char*>(asset->getBuffer(true)), asset->getLength());

    Json::CharReaderBuilder builder;
    Json::Value root;
    std::string errs;
    std::unique_ptr<Json::CharReader> reader(builder.newCharReader());
    if (!reader->parse(json_content.c_str(), json_content.c_str() + json_content.size(), &root,
                       &errs)) {
        return Error() << "Failed to parse " << config_path_in_apk << ": " << errs;
    }

    if (!root.isMember("tenants") || !root["tenants"].isArray()) {
        // No tenants defined in the config, which is a valid case.
        return {};
    }

    int i = 0;
    for (const auto& tenant : root["tenants"]) {
        if (!tenant.isMember("package") || tenant["package"].asString() != "apk" ||
            !tenant.isMember("name") || !tenant["name"].isString()) {
            // Not an APK tenant, skip.
            continue;
        }

        const std::string package_name = tenant["name"].asString();
        auto apk_path_result = get_apk_path_from_package_name(package_name);
        if (!apk_path_result.ok()) {
            return Error() << "Failed to get apk path for package '" << package_name
                           << "': " << apk_path_result.error();
        }
        std::string apk_path = *apk_path_result;
        auto apk_fd = OR_RETURN(open_file(apk_path, O_RDONLY));

        std::string idsig_path = work_dir + "/tenant_idsig_" + std::to_string(i++) + ".idsig";
        ScopedFileDescriptor idsig_fd = OR_RETURN(open_file(idsig_path, O_CREAT | O_RDWR));
        ScopedAStatus ret = service.createOrUpdateIdsigFile(apk_fd, idsig_fd);
        if (!ret.isOk()) {
            return Error() << "Failed to create or update idsig file: " << idsig_path;
        }

        app_config->tenantApks.push_back(std::move(apk_fd));
        app_config->tenantIdsigs.push_back(std::move(idsig_fd));
    }

    return {};
}

// The payload for the VM can be specified either as a path to a config file in the APK, or the
// name of a native binary in the APK.
struct VmPayload {
    enum class Type {
        kBinaryName,
        kConfigPath,
    };
    Type type;
    std::string value;

    static VmPayload asBinaryName(std::string name) { return {Type::kBinaryName, std::move(name)}; }
    static VmPayload asConfigPath(std::string path) { return {Type::kConfigPath, std::move(path)}; }
};

// Construct VirtualMachineAppConfig for a Microdroid-based VM named `vm_name` that executes a
// shared library named `paylaod_binary_name` in the apk `main_apk_path`.
Result<VirtualMachineAppConfig> create_vm_config(IVirtualizationService& service,
                                                 const std::string& work_dir,
                                                 const std::string& vm_name,
                                                 const std::string& main_apk_path,
                                                 const VmPayload& payload, bool debuggable,
                                                 bool protected_vm, int32_t memory_mib) {
    ScopedFileDescriptor main_apk = OR_RETURN(open_file(main_apk_path, O_RDONLY));
    ScopedFileDescriptor idsig =
            OR_RETURN(create_or_update_idsig_file(service, work_dir, main_apk));
    ScopedFileDescriptor instance =
            OR_RETURN(create_instance_image_file_if_needed(service, work_dir));
    ScopedFileDescriptor encryptedstore =
            OR_RETURN(create_encryptedstore_image_file_if_needed(service, work_dir));
    std::array<uint8_t, 64> instance_id;
    OR_RETURN(get_or_allocate_instance_id(service, work_dir, &instance_id));

    VirtualMachineAppConfig app_config;
    app_config.name = vm_name;
    app_config.apk = std::move(main_apk);
    app_config.idsig = std::move(idsig);
    app_config.instanceImage = std::move(instance);
    app_config.encryptedStorageImage = std::move(encryptedstore);
    app_config.instanceId = instance_id;
    if (debuggable) {
        app_config.debugLevel = VirtualMachineAppConfig::DebugLevel::FULL;
    }
    app_config.protectedVm = protected_vm;
    app_config.memoryMib = memory_mib;

    if (payload.type == VmPayload::Type::kConfigPath) {
        OR_RETURN(add_tenant_apks_from_config(service, main_apk_path, payload.value, work_dir,
                                              &app_config));
    }

    // There are two ways to specify the payload. The simpler way is by specifying the name of the
    // payload binary as shown below. The other way (which is allowed only to system-level VMs) is
    // by passing the path to the JSON file in the main APK which has detailed specification about
    // what to load in Microdroid. See packages/modules/Virtualization/compos/apk/assets/*.json as
    // examples.
    //
    // For multi tenancy case, multiple tenants are only supported through
    // config JSON file
#if AVF_ENABLE_ADVANCE_MULTITENANCY
    app_config.payload = payload.value;
#else
    VirtualMachinePayloadConfig payloadConfig;
    payloadConfig.payloadBinaryName = payload.value;

    app_config.payload = std::move(payloadConfig);
#endif

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
    ScopedFileDescriptor dump_dt_fd(-1);

    ScopedAStatus ret =
            service.createVm(config, console_out_fd, console_in_fd, log_fd, dump_dt_fd, &vm);
    if (!ret.isOk()) {
        return Error() << "Failed to create VM: " << ret.getMessage();
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

    ScopedAStatus onGuestAgentRegistered(int32_t, const std::shared_ptr<IGuestAgent>&) {
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

    auto param = std::make_unique<std::shared_ptr<IVirtualMachine>>(std::move(vm));
    auto paramDeleteFd = [](void* param) {
        delete static_cast<std::shared_ptr<IVirtualMachine>*>(param);
    };

    AIBinder* binder = ARpcSession_setupPreconnectedClient(
            session.get(),
            [](void* param) {
                IVirtualMachine* vm = static_cast<std::shared_ptr<IVirtualMachine>*>(param)->get();
                ScopedFileDescriptor sock_fd;
                ScopedAStatus ret = vm->connectVsock(ITestService::PORT, &sock_fd);
                if (!ret.isOk()) {
                    return -1;
                }
                return sock_fd.release();
            },
            param.release(), paramDeleteFd);
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
    std::string work_dir_path("/data/local/tmp/vm_demo/");
    if (mkdir(work_dir_path.c_str(), 0700) == -1 && errno != EEXIST) {
        return ErrnoError() << "failed to create working directory " << work_dir_path.c_str();
    }

    // Step 1: connect to the virtualizationservice
    unique_fd fd = OR_RETURN(get_service_fd());
    std::shared_ptr<IVirtualizationService> service = OR_RETURN(connect_service(fd.get()));

    // Step 2: create vm config
    VirtualMachineAppConfig app_config = OR_RETURN(
            create_vm_config(*service, work_dir_path, "my_vm",
                             "/data/local/tmp/MicrodroidTestHelperApp.apk",
#if AVF_ENABLE_ADVANCE_MULTITENANCY
                             VmPayload::asConfigPath("assets/vm_config_multi_tenants.json"),
#else
                             VmPayload::asBinaryName("MicrodroidTestNativeLib.so"),
#endif
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
