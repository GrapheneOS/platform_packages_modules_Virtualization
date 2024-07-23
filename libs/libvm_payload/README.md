# VM Payload API

This directory contains the definition of the VM Payload API. This is a native
API, exposed as a set of C functions, available to payload code running inside a
[Microdroid](https://android.googlesource.com/platform/packages/modules/Virtualization/+/refs/heads/main/build/microdroid/README.md)
VM.

Note that only native code is supported in Microdroid, so no Java APIs are
available in the VM, and only 64 bit code is supported.

To create a VM and run the payload from Android see the [AVF Java
APIs](../libs/framework-virtualization/README.md).

## Entry point

The payload should be packaged as one (or more) .so files inside the app's APK -
under the `lib/<ABI>` directory, like other JNI code.

The primary .so, which is specified as part of the VM configuration via
[VirtualMachineConfig.Builder#setPayloadBinaryPath](https://android.googlesource.com/platform/packages/modules/Virtualization/+/refs/heads/main/libs/framework-virtualization/src/android/system/virtualmachine/VirtualMachineConfig.java),
must define the entry point for the payload.

This entry point is a C function called `AVmPayload_main()`, as declared in
[vm_main.h](include/vm_main.h). (In C++ this must be defined as `extern "C"`.)

## API header

The functions available to the payload once it starts are declared in
[vm_payload.h](include/vm_payload.h).

### Linking

In the Android build system, the payload binary should be built with
`libvm_payload#current` specified as one of the `shared_libs`; this links
against a stub `libvm_payload.so`, where the dependencies will be satisfied at
runtime from the real `libvm_payload.so` hosted within the Microdroid VM.

See `MicrodroidTestNativeLib` in the [test
APK](https://android.googlesource.com/platform/packages/modules/Virtualization/+/refs/heads/main/tests/testapk/Android.bp)
for an example.

In other build systems a similar stub `libvm_payload.so` can be built using
[stub.c](stub/stub.c) and the [linker script](libvm_payload.map.txt).

## Available NDK APIs

In addition to the VM Payload APIs, a small subset of the [Android
NDK](https://developer.android.com/ndk) can be used by the payload.

This subset consists of:
- The [standard C library](https://developer.android.com/ndk/guides/stable_apis#c_library).
- The [Logging APIs](https://developer.android.com/ndk/guides/stable_apis#logging).
- The [NdkBinder
  API](https://developer.android.com/ndk/reference/group/ndk-binder). However
  note that the payload can only host a binder server via
  `AVmPayload_runVsockRpcServer`, defined in
  [vm_payload.h](include/vm_payload.h), rather than
  `AServiceManager_addService`, and cannot connect to any binder server. Passing
  file descriptors to and from the VM is not supported.

## C++

C++ can be used, but you will need to include the C++ runtime in your APK along
with your payload, either statically linked (if
[appropriate](https://developer.android.com/ndk/guides/cpp-support#sr)) or as a
separate .so.

The same is true for other languages such as Rust.

See [AIDL
backends](https://source.android.com/docs/core/architecture/aidl/aidl-backends)
for information on using AIDL with the NDK Binder from C++.

## Rust

A Rust wrapper library for the VM Payload API is available (as an rlib) for VM
payloads written in Rust.

This wrapper is not guaranteed to be stable; we may change it in future
versions. But payload code built using it will depend only on the C VM Payload
API and the NDK APIs that are available to the payload, so should run unchanged
on future versions.

See [wrapper/lib.rs](wrapper/lib.rs) and `libvm_payload_rs` in
[Android.bp](Android.bp).
