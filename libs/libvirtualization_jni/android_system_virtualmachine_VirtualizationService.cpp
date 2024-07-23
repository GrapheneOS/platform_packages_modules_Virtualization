/*
 * Copyright 2022 The Android Open Source Project
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

#define LOG_TAG "VirtualizationService"

#include <android-base/unique_fd.h>
#include <android/avf_cc_flags.h>
#include <android/binder_ibinder_jni.h>
#include <jni.h>
#include <log/log.h>
#include <poll.h>

#include <string>

#include "common.h"

using namespace android::base;

static constexpr const char VIRTMGR_PATH[] = "/apex/com.android.virt/bin/virtmgr";
static constexpr size_t VIRTMGR_THREADS = 2;

extern "C" JNIEXPORT jint JNICALL
Java_android_system_virtualmachine_VirtualizationService_nativeSpawn(
        JNIEnv* env, [[maybe_unused]] jclass clazz) {
    unique_fd serverFd, clientFd;
    if (!Socketpair(SOCK_STREAM, &serverFd, &clientFd)) {
        env->ThrowNew(env->FindClass("android/system/virtualmachine/VirtualMachineException"),
                      ("Failed to create socketpair: " + std::string(strerror(errno))).c_str());
        return -1;
    }

    unique_fd waitFd, readyFd;
    if (!Pipe(&waitFd, &readyFd, 0)) {
        env->ThrowNew(env->FindClass("android/system/virtualmachine/VirtualMachineException"),
                      ("Failed to create pipe: " + std::string(strerror(errno))).c_str());
        return -1;
    }

    if (fork() == 0) {
        // Close client's FDs.
        clientFd.reset();
        waitFd.reset();

        auto strServerFd = std::to_string(serverFd.get());
        auto strReadyFd = std::to_string(readyFd.get());

        execl(VIRTMGR_PATH, VIRTMGR_PATH, "--rpc-server-fd", strServerFd.c_str(), "--ready-fd",
              strReadyFd.c_str(), NULL);
    }

    // Close virtmgr's FDs.
    serverFd.reset();
    readyFd.reset();

    // Wait for the server to signal its readiness by closing its end of the pipe.
    char buf;
    int ret = read(waitFd.get(), &buf, sizeof(buf));
    if (ret < 0) {
        env->ThrowNew(env->FindClass("android/system/virtualmachine/VirtualMachineException"),
                      "Failed to wait for VirtualizationService to be ready");
        return -1;
    } else if (ret < 1) {
        env->ThrowNew(env->FindClass("java/lang/SecurityException"),
                      "Virtmgr didn't send any data through pipe. Please consider checking if "
                      "android.permission.MANAGE_VIRTUAL_MACHINE permission is granted");
        return -1;
    }

    return clientFd.release();
}

extern "C" JNIEXPORT jobject JNICALL
Java_android_system_virtualmachine_VirtualizationService_nativeConnect(JNIEnv* env,
                                                                       [[maybe_unused]] jobject obj,
                                                                       int clientFd) {
    RpcSessionHandle session;
    ARpcSession_setFileDescriptorTransportMode(session.get(),
                                               ARpcSession_FileDescriptorTransportMode::Unix);
    ARpcSession_setMaxIncomingThreads(session.get(), VIRTMGR_THREADS);
    // SAFETY - ARpcSession_setupUnixDomainBootstrapClient does not take ownership of clientFd.
    auto client = ARpcSession_setupUnixDomainBootstrapClient(session.get(), clientFd);
    return AIBinder_toJavaBinder(env, client);
}

extern "C" JNIEXPORT jboolean JNICALL
Java_android_system_virtualmachine_VirtualizationService_nativeIsOk(JNIEnv* env,
                                                                    [[maybe_unused]] jobject obj,
                                                                    int clientFd) {
    /* Setting events=0 only returns POLLERR, POLLHUP or POLLNVAL. */
    struct pollfd pfds[] = {{.fd = clientFd, .events = 0}};
    if (poll(pfds, /*nfds*/ 1, /*timeout*/ 0) < 0) {
        env->ThrowNew(env->FindClass("android/system/virtualmachine/VirtualMachineException"),
                      ("Failed to poll client FD: " + std::string(strerror(errno))).c_str());
        return false;
    }
    return pfds[0].revents == 0;
}

extern "C" JNIEXPORT jboolean JNICALL
Java_android_system_virtualmachine_VirtualizationService_nativeIsVendorModulesFlagEnabled(
        [[maybe_unused]] JNIEnv* env, [[maybe_unused]] jobject obj) {
    return android::virtualization::IsVendorModulesFlagEnabled();
}
