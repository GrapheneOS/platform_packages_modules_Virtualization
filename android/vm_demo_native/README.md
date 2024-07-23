# Microdroid Demo app in C++

This app is a demonstration of how to create a VM and run payload in it, in C++.

## Restriction

This is for VMs that are part of the platform itself. Specifically, this C++ example is for cases
like creating and using a VM from a HAL process.

For non-system-level VMs, you must use the Java APIs from an Android app. See the [Java demo
app](../MicrodroidDemoApp/README.md).

## Building

```sh
source build/envsetup.sh
choosecombo 1 aosp_arm64 userdebug
m MicrodroidTestApp
m vm_demo_native
```

`MicrodroidTestApp` is the application what will be running in the VM. Actually, we will run a
native shared library `MicrodroidTestNativeLib.so` from the APK.

`vm_demo_native` runs on the host (i.e. Android). Its job is to start the VM and connect to the
native shared lib and do some work using the lib. Specifically, we will call an AIDL method
`addInteger` which adds two integers and returns the sum. The computation will be done in the VM.

## Installing

```sh
adb push out/target/product/generic_arm64/testcases/MicrodroidTestApp/arm64/MicrodroidTestApp.apk \
  /data/local/tmp/
adb push out/target/product/generic_arm64/system/bin/vm_demo_native /data/local/tmp/
```

## Running

```sh
adb root
adb shell setenforce 0
adb shell /data/local/tmp/vm_demo_native
```

Rooting and selinux disabling are required just because there's no sepolicy configured for this demo
application. For production, you need to set the sepolicy up correctly. You may use
`system/sepolicy/private/composd.te` (specifically, the macro `virtualizationservice_use`) as a
reference.

## Expected output

```sh
[2023-05-10T23:45:54.904181191+09:00 INFO  crosvm] crosvm started.
[2023-05-10T23:45:54.906048663+09:00 INFO  crosvm] CLI arguments parsed.
...
The answer from VM is 30
[    1.996707][   T57] microdroid_manager[57]: task successfully finished
...
[2023-05-10T23:45:58.263614461+09:00 INFO  crosvm] exiting with success
Done
```
