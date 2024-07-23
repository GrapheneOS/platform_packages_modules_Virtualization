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

#include <android-base/logging.h>
#include <android-base/result.h>
#include <android/dlext.h>
#include <dlfcn.h>

#include <cstdlib>
#include <iostream>
#include <string>

#include "vm_main.h"

using android::base::Error;
using android::base::Result;

extern "C" {
enum {
    ANDROID_NAMESPACE_TYPE_REGULAR = 0,
    ANDROID_NAMESPACE_TYPE_ISOLATED = 1,
    ANDROID_NAMESPACE_TYPE_SHARED = 2,
};

extern struct android_namespace_t* android_create_namespace(
        const char* name, const char* ld_library_path, const char* default_library_path,
        uint64_t type, const char* permitted_when_isolated_path,
        struct android_namespace_t* parent);

extern bool android_link_namespaces(struct android_namespace_t* from,
                                    struct android_namespace_t* to,
                                    const char* shared_libs_sonames);
} // extern "C"

static Result<void*> load(const std::string& libname);

constexpr char entrypoint_name[] = "AVmPayload_main";

static constexpr const char* kAllowedLibs[] = {
        "libc.so",   "libm.so",          "libdl.so",         "libdl_android.so",
        "liblog.so", "libvm_payload.so", "libbinder_ndk.so", "libbinder_rpc_unstable.so",
};

int main(int argc, char* argv[]) {
    if (argc != 2) {
        std::cout << "Usage:\n";
        std::cout << "    " << argv[0] << " LIBNAME\n";
        return EXIT_FAILURE;
    }

    android::base::InitLogging(argv);

    const char* libname = argv[1];
    auto handle = load(libname);
    if (!handle.ok()) {
        LOG(ERROR) << "Failed to load " << libname << ": " << handle.error().message();
        return EXIT_FAILURE;
    }

    AVmPayload_main_t* entry = reinterpret_cast<decltype(entry)>(dlsym(*handle, entrypoint_name));
    if (entry == nullptr) {
        LOG(ERROR) << "Failed to find entrypoint `" << entrypoint_name << "`: " << dlerror();
        return EXIT_FAILURE;
    }

    return entry();
}

// Create a new linker namespace whose search path is set to the directory of the library. Then
// load it from there. Returns the handle to the loaded library if successful. Returns nullptr
// if failed.
Result<void*> load(const std::string& libname) {
    // Parent as nullptr means the default namespace
    android_namespace_t* parent = nullptr;
    // The search paths of the new namespace are isolated to restrict system private libraries.
    const uint64_t type = ANDROID_NAMESPACE_TYPE_ISOLATED;
    // The directory of the library is appended to the search paths
    const std::string libdir = libname.substr(0, libname.find_last_of("/"));
    const char* ld_library_path = libdir.c_str();
    const char* default_library_path = libdir.c_str();

    android_namespace_t* new_ns = nullptr;
    new_ns = android_create_namespace("microdroid_app", ld_library_path, default_library_path, type,
                                      /* permitted_when_isolated_path */ nullptr, parent);
    if (new_ns == nullptr) {
        return Error() << "Failed to create linker namespace: " << dlerror();
    }

    std::string libs;
    for (const char* lib : kAllowedLibs) {
        if (!libs.empty()) libs += ':';
        libs += lib;
    }
    if (!android_link_namespaces(new_ns, nullptr, libs.c_str())) {
        return Error() << "Failed to link namespace: " << dlerror();
    }

    const android_dlextinfo info = {
            .flags = ANDROID_DLEXT_USE_NAMESPACE,
            .library_namespace = new_ns,
    };
    if (auto ret = android_dlopen_ext(libname.c_str(), RTLD_NOW, &info); ret) {
        return ret;
    } else {
        return Error() << "Failed to dlopen: " << dlerror();
    }
}
