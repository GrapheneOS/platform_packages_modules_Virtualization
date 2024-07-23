# Microdroid

Microdroid is a (very) lightweight version of Android that is intended to run on
on-device virtual machines. It is built from the same source code as the regular
Android, but it is much smaller; no system server, no HALs, no GUI, etc. It is
intended to host headless & native workloads only.

## Prerequisites

Any 64-bit target (either x86\_64 or arm64) is supported. 32-bit target is not
supported.

The only remaining requirement is that `com.android.virt` APEX has to be
pre-installed. To do this, add the following line in your product makefile.

```make
$(call inherit-product, packages/modules/Virtualization/build/apex/product_packages.mk)
```

Build the target product after adding the line, and flash it. This step needs
to be done only once for the target.

If you are using Pixel 6 and beyond or Cuttlefish (`aosp_cf_x86_64_phone`)
adding above line is not necessary as it's already done.

## Building and installing microdroid

Microdroid is part of the `com.android.virt` APEX. To build it and install to
the device:

```sh
banchan com.android.virt aosp_arm64
UNBUNDLED_BUILD_SDKS_FROM_SOURCE=true m apps_only dist
adb install out/dist/com.android.virt.apex
adb reboot
```

If your target is x86\_64 (e.g. `aosp_cf_x86_64_phone`), replace `aosp_arm64`
with `aosp_x86_64`.

## Building an app

A [vm
payload](https://android.googlesource.com/platform/packages/modules/Virtualization/+/refs/heads/main/libs/libvm_payload/)
is a shared library file that gets executed in microdroid. It is packaged as
part of an Android application.  The library should have an entry point
`AVmPayload_main` as shown below:

```C++
extern "C" int AVmPayload_main() {
  printf("Hello Microdroid!\n");
}
```

Then build it as a shared library:

```
cc_library_shared {
  name: "MyMicrodroidPayload",
  srcs: ["**/*.cpp"],
  sdk_version: "current",
}
```

Embed the shared library file in an APK:

```
android_app {
  name: "MyApp",
  srcs: ["**/*.java"],
  jni_libs: ["MyMicrodroidPayload"],
  use_embedded_native_libs: true,
  sdk_version: "current",
}
```

Finally, you build the APK.

```sh
TARGET_BUILD_APPS=MyApp m apps_only dist
```

## Running the VM payload on microdroid

First of all, install the APK to the target device.

```sh
adb install out/dist/MyApp.apk
```

There are two ways start a VM and run the payload in it.

* By manually invoking the `vm` tool via `adb shell`.
* Calling APIs programmatically in the Java app.

### Using `vm` tool

Execute the following commands to launch a VM. The VM will boot to microdroid
and then automatically execute your payload (the shared library
`MyMicrodroidPayload.so`).

```sh
TEST_ROOT=/data/local/tmp/virt
adb shell /apex/com.android.virt/bin/vm run-app \
--log $TEST_ROOT/log.txt \
--console $TEST_ROOT/console.txt \
PATH_TO_YOUR_APP \
$TEST_ROOT/MyApp.apk.idsig \
$TEST_ROOT/instance.img \
--instance-id-file $TEST_ROOT/instance_id \
--payload-binary-name MyMicrodroidPayload.so
```

`ALL_CAP`s below are placeholders. They need to be replaced with correct
values:

* `PACKAGE_NAME_OF_YOUR_APP`: package name of your app (e.g. `com.acme.app`).
* `PATH_TO_YOUR_APP`: path to the installed APK on the device. Can be obtained
  via the following command.
  ```sh
  adb shell pm path PACKAGE_NAME_OF_YOUR_APP
  ```
  It shall report a cryptic path similar to `/data/app/~~OgZq==/com.acme.app-HudMahQ==/base.apk`.

The console output from the VM is stored to `$TEST_ROOT/console.txt` and logcat
is stored to `$TEST_ROOT/log.txt` file for debugging purpose. If you omit
`--log` or `--console` option, the console output will be emitted to the
current console and the logcat logs are sent to the main logcat in Android.

Stopping the VM can be done by pressing `Ctrl+C`.

### Using the APIs

Use the [Android Virtualization Framework Java
APIs](https://android.googlesource.com/platform/packages/modules/Virtualization/+/refs/heads/main/libs/framework-virtualization/README.md)
in your app to create a microdroid VM and run payload in it. The APIs are currently
@SystemApi, and only available to preinstalled apps.

If you are looking for an example usage of the APIs, you may refer to the [demo
app](https://android.googlesource.com/platform/packages/modules/Virtualization/+/refs/heads/main/android/MicrodroidDemoApp/).


## Running Microdroid with vendor image

With using `vm` tool, execute the following commands to launch a VM with vendor
partition.

```sh
adb shell /apex/com.android.virt/bin/vm run-microdroid \
--vendor $VENDOR_IMAGE
```

### Verification of vendor image

Since vendor image of Microdroid is not part of `com.android.virt` APEX, the
verification process of vendor partition is different from others.

Vendor image uses its hashtree digest for the verifying its data, generated
by `add_hashtree_footer` in `avbtool`. The value could be seen with following
command:

```sh
avbtool info_image --image $VENDOR_IMAGE
```

Fixed path in VM for vendor hashtree digest is written in [fstab.microdroid].
During first stage init of VM, [dm-verity] is set up based on vendor hashtree
digest by reading [fstab.microdroid].

For non-pVM, virtualizationmanager creates [DTBO] containing vendor hashtree
digest, and passes to the VM via crosvm option. The vendor hashtree digest is
obtained by virtualizationmanager from the host Android DT under
`/avf/reference/`, which may be populated by the [bootloader].

For pVM, VM reference DT included in [pvmfw config data] is additionally used
for validating vendor hashtree digest. [Bootloader][bootloader] should append
vendor hashtree digest into VM reference DT based on [fstab.microdroid]. Vendor
hashtree digest could be appended as property into descriptors in host Android's
vendor image by [Makefile] when Microdroid vendor image module is defined, so
that a [bootloader] can extract the value and populate into VM reference DT.

[fstab.microdroid]: fstab.microdroid
[dm-verity]: https://source.android.com/docs/security/features/verifiedboot/dm-verity
[DTBO]: https://android.googlesource.com/platform/external/dtc/+/refs/heads/main/Documentation/dt-object-internal.txt
[pvmfw config data]: ../guest/pvmfw/README.md#configuration-data-format
[bootloader]: https://source.android.com/docs/core/architecture/bootloader
[Makefile]: https://cs.android.com/android/platform/superproject/main/+/main:build/make/core/Makefile

## Debugging Microdroid

Refer to [Debugging protected VMs](../docs/debug/README.md).
