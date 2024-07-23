/*
 * Copyright 2021 The Android Open Source Project
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

#define LOG_TAG "VirtualMachine"

#include <aidl/android/system/virtualizationservice/IVirtualMachine.h>
#include <android-base/scopeguard.h>
#include <android-base/strings.h>
#include <android/binder_auto_utils.h>
#include <android/binder_ibinder_jni.h>
#include <fcntl.h>
#include <jni.h>
#include <log/log.h>
#include <nativehelper/JNIHelp.h>
#include <nativehelper/JNIPlatformHelp.h>
#include <nativehelper/ScopedLocalRef.h>
#include <pty.h>
#include <string.h>
#include <sys/stat.h>
#include <termios.h>
#include <unistd.h>

#include <binder_rpc_unstable.hpp>
#include <string>
#include <tuple>

#include "common.h"

namespace {

void throwIOException(JNIEnv *env, const std::string &msg) {
    jniThrowException(env, "java/io/IOException", msg.c_str());
}

} // namespace

extern "C" JNIEXPORT jobject JNICALL
Java_android_system_virtualmachine_VirtualMachine_nativeConnectToVsockServer(
        JNIEnv *env, [[maybe_unused]] jclass clazz, jobject vmBinder, jint port) {
    using aidl::android::system::virtualizationservice::IVirtualMachine;
    using ndk::ScopedFileDescriptor;
    using ndk::SpAIBinder;

    auto vm = IVirtualMachine::fromBinder(SpAIBinder{AIBinder_fromJavaBinder(env, vmBinder)});

    std::tuple args{env, vm.get(), port};
    using Args = decltype(args);

    auto requestFunc = [](void *param) {
        auto [env, vm, port] = *static_cast<Args *>(param);

        ScopedFileDescriptor fd;
        if (auto status = vm->connectVsock(port, &fd); !status.isOk()) {
            env->ThrowNew(env->FindClass("android/system/virtualmachine/VirtualMachineException"),
                          ("Failed to connect vsock: " + status.getDescription()).c_str());
            return -1;
        }

        // take ownership
        int ret = fd.get();
        *fd.getR() = -1;

        return ret;
    };

    RpcSessionHandle session;
    // We need a thread pool to be able to support linkToDeath, or callbacks
    // (b/268335700). These threads are currently created eagerly, so we don't
    // want too many. The number 1 is chosen after some discussion, and to match
    // the server-side default (mMaxThreads on RpcServer).
    ARpcSession_setMaxIncomingThreads(session.get(), 1);
    auto client = ARpcSession_setupPreconnectedClient(session.get(), requestFunc, &args);
    return AIBinder_toJavaBinder(env, client);
}

extern "C" JNIEXPORT void JNICALL
Java_android_system_virtualmachine_VirtualMachine_nativeOpenPtyRawNonblock(
        JNIEnv *env, [[maybe_unused]] jclass clazz, jobject resultCallback) {
    int pm, ps;
    // man openpty says: "Nobody knows how much space should be reserved for name."
    // but on modern Linux the format of the pts name is always `/dev/pts/[0-9]+`
    // Realistically speaking, a buffer of 32 bytes leaves us with 22 digits for the pts number,
    // which should be more than enough.
    // NOTE: bionic implements openpty() with internal name buffer of size 32, musl 20.
    char name[32];
    if (openpty(&pm, &ps, name, nullptr, nullptr)) {
        throwIOException(env, "openpty(): " + android::base::ErrnoNumberAsString(errno));
        return;
    }
    fcntl(pm, F_SETFD, FD_CLOEXEC);
    fcntl(ps, F_SETFD, FD_CLOEXEC);
    name[sizeof(name) - 1] = '\0';
    // Set world RW so adb shell can talk to the pts.
    chmod(name, 0666);

    if (int flags = fcntl(pm, F_GETFL, 0); flags < 0) {
        throwIOException(env, "fcntl(F_GETFL): " + android::base::ErrnoNumberAsString(errno));
        return;
    } else if (fcntl(pm, F_SETFL, flags | O_NONBLOCK) < 0) {
        throwIOException(env, "fcntl(F_SETFL): " + android::base::ErrnoNumberAsString(errno));
        return;
    }

    android::base::ScopeGuard cleanup_handler([=] {
        close(ps);
        close(pm);
    });

    struct termios tio;
    if (tcgetattr(pm, &tio)) {
        throwIOException(env, "tcgetattr(): " + android::base::ErrnoNumberAsString(errno));
        return;
    }
    cfmakeraw(&tio);
    if (tcsetattr(pm, TCSANOW, &tio)) {
        throwIOException(env, "tcsetattr(): " + android::base::ErrnoNumberAsString(errno));
        return;
    }

    jobject mfd = jniCreateFileDescriptor(env, pm);
    if (mfd == nullptr) {
        return;
    }
    jobject sfd = jniCreateFileDescriptor(env, ps);
    if (sfd == nullptr) {
        return;
    }
    size_t len = strlen(name);
    ScopedLocalRef<jbyteArray> ptsName(env, env->NewByteArray(len));
    if (ptsName.get() != nullptr) {
        env->SetByteArrayRegion(ptsName.get(), 0, len, (jbyte *)name);
    }
    ScopedLocalRef<jclass> callback_class(env, env->GetObjectClass(resultCallback));
    jmethodID mid = env->GetMethodID(callback_class.get(), "apply",
                                     "(Ljava/io/FileDescriptor;Ljava/io/FileDescriptor;[B)V");
    if (mid == nullptr) {
        return;
    }

    env->CallVoidMethod(resultCallback, mid, mfd, sfd, ptsName.get());
    // FD ownership is transferred to the callback, reset the auto-close hander.
    cleanup_handler.Disable();
}
