# Microdroid kernel

This directory contains prebuilt images of the Linux kernel that is used in
Microdroid. The kernel is built from the same source tree as Generic Kernel
Image (GKI), but with a different config where most of the config items are
turned off to make the kernel fast & slim.

## How to build the Microdroid kernels

### Checkout the GKI source code.

```bash
repo init -u https://android.googlesource.com/kernel/manifest -b common-android15-6.6
repo sync
```

### Build the Microdroid kernels manually

For ARM64
```bash
tools/bazel clean
tools/bazel run --config=fast //common:kernel_aarch64_microdroid_dist -- --destdir=out/dist
```

For x86\_64,
```bash
tools/bazel clean
tools/bazel run --config=fast //common:kernel_x86_64_microdroid_dist -- --destdir=out/dist
```

Note that
[`--config=fast`](https://android.googlesource.com/kernel/build/+/refs/heads/main/kleaf/docs/fast.md)
is not mandatory, but will make your build much faster.

The build may fail in case you are doing an incremental build and the config has changed (b/257288175). Until that issue
is fixed, do the clean build by invoking `tools/bazel clean` before the build command.

### Change the kernel configs

For ARM64
```bash
tools/bazel run //common:kernel_aarch64_microdroid_config -- menuconfig
```

For x86\_64
```bash
tools/bazel run //common:kernel_x86_64_microdroid_config -- menuconfig
```

## How to update Microdroid kernel prebuilts

### For manually built kernels (only for your own development)

Copy the built kernel image to the Android source tree directly, and build the virt APEX.

For ARM64,
```bash
cp out/dist/Image <android_checkout>/packages/modules/Virtualization/guest/kernel/android15-6.6/arm64/kernel-6.6
```

For x86\_64,
```bash
cp out/dist/bzImage <android_checkout>/packages/modules/Virtualization/guest/kernel/android15-6.6/x86_64/kernel-6.6
```

### For new kernel versions added that did not exist before

Make a new directory with the Android version and the kernel version.

```bash
mkdir -p <android_checkout>/packages/modules/Virtualization/guest/kernel/android17-6.18/
```

Then add a directory for each architecture (arm64 | x86_64) and copy the
kernels.

Modify the Android.bp `select` statements to include the new kernels.

Finally, modify
https://source.corp.google.com/h/googleplex-android/platform/superproject/main/+/main:build/release/flag_values/trunk_staging/RELEASE_AVF_MICRODROID_KERNEL_VERSION.textproto;l=3?q=RELEASE_AVF_MICRODROID_KERNEL_VERSION&sq=git:googleplex-android%2Fplatform%2Fsuperproject%2Fmain@refs%2Fheads%2Fmain

## Debugging microdroid using GDB

Debugging documentation located at:

```bash
<android_checkout>/packages/modules/Virtualization/docs/debug/gdb_kernel.md
```

### For official updates

Use the `download_from_ci` script to automatically fetch the built images from
a specific `<build_id>` and make commits with nice history in the message.

```bash
cd <android_checkout>/packages/modules/Virtualization
repo start <topic_name>
cd <kernel_checkout>
ANDROID_BUILD_TOP=<android_checkout> ./build/kernel/gki/download_from_ci  --update-microdroid -b <bug_id> <build_id>
cd <android_checkout>/packages/modules/Virtualization
repo upload .
```
