# Getting started with Android Virtualization Framework

## Step 1: Prepare a device

We support the following devices:

* aosp\_panther (Pixel 7)
* aosp\_cheetah (Pixel 7 Pro)
* aosp\_oriole (Pixel 6)
* aosp\_raven (Pixel 6 Pro)
* aosp\_felix (Pixel Fold)
* aosp\_tangopro (Pixel Tablet)
* aosp\_cf\_x86\_64\_phone (Cuttlefish a.k.a. Cloud Android). Follow [this
  instruction](https://source.android.com/docs/setup/create/cuttlefish-use) to
  use.

### Note on Pixel 6 and 6 Pro
AVF is shipped in Pixel 6 and 6 Pro, but isn't enabled by default. To enable
it, follow the instructions below:

1. If the device is running Android 13 or earlier, upgrade to Android 14.

1. Once upgraded to Android 14, execute the following command to enable pKVM.
   ```shell
   adb reboot bootloader
   fastboot flashing unlock
   fastboot oem pkvm enable
   fastboot reboot
   ```
### Note on Cuttlefish
Cuttlefish does not support protected VMs. Only non-protected VMs are
supported.

## Step 2: Build Android image

This step is optional unless you want to build AVF by yourself or try the
in-development version of AVF.

AVF is implemented as an APEX named `com.android.virt`. However, in order for
you to install it to your device (be it Pixel or Cuttlefish), you first need to
re-build the entire Android from AOSP. This is because the official Android
build you have in your device is release-key signed and therefore you can't
install your custom-built AVF APEX to it - because it is test-key signed.

### Pixel

1. [Download](https://source.android.com/docs/setup/download/downloading)
   source code from AOSP. Use the `main` branch.

1. [Download](https://developers.google.com/android/blobs-preview) the preview
   vendor blob that matches your device.

1. [Build](https://source.android.com/docs/setup/build/building) the `aosp_`
   variant of your device. For example, if your device is Pixel 7 (`panther`),
   build `aosp_panther`.

1. [Flash](https://source.android.com/docs/setup/build/running) the built
   images to the device.


### Cuttlefish

1. [Download](https://source.android.com/docs/setup/download/downloading)
   source code from AOSP. Use the `main` branch.

1. Build Cuttlefish:
   ```shell
   source build/envsetup.sh
   lunch aosp_cf_x86_64_phone-userdebug
   m
   ```

1. Run Cuttlefish:
   ```shell
   cvd start
   ```

## Step 3: Build AVF

Then you can repeat building and installing AVF to the device as follows:

1. Build the AVF APEX.
   ```sh
   banchan com.android.virt aosp_arm64
   UNBUNDLED_BUILD_SDKS_FROM_SOURCE=true m apps_only dist
   ```
   Replace `aosp_arm64` with `aosp_x86_64` if you are building for Cuttlefish.

1. Install the AVF APEX to the device.
   ```sh
   adb install out/dist/com.android.virt.apex
   adb reboot; adb wait-for-device
   ```

## Step 4: Run a Microdroid VM

[Microdroid](../../build/microdroid/README.md) is a lightweight version of Android
that is intended to run on pVM. You can run a Microdroid-based VM with an empty
payload using the following command:

```shell
packages/modules/Virtualization/vm/vm_shell.sh start-microdroid --auto-connect -- --protected
```

You will see the log messages like the below.

```
found path /apex/com.android.virt/app/EmptyPayloadAppGoogle@MASTER/EmptyPayloadAppGoogle.apk
creating work dir /data/local/tmp/microdroid/7CI6QtktSluD3OZgv
apk.idsig path: /data/local/tmp/microdroid/7CI6QtktSluD3OZgv/apk.idsig
instance.img path: /data/local/tmp/microdroid/7CI6QtktSluD3OZgv/instance.img
Created VM from "/apex/com.android.virt/app/EmptyPayloadAppGoogle@MASTER/EmptyPayloadAppGoogle.apk"!PayloadConfig(VirtualMachinePayloadConfig { payloadBinaryName: "MicrodroidEmptyPayloadJniLib.so" }) with CID 2052, state is STARTING.
[2023-07-07T14:50:43.420766770+09:00 INFO  crosvm] crosvm started.
[2023-07-07T14:50:43.422545090+09:00 INFO  crosvm] CLI arguments parsed.
[2023-07-07T14:50:43.440984015+09:00 INFO  crosvm::crosvm::sys::unix::device_helpers] Trying to attach block device: /proc/self/fd/49
[2023-07-07T14:50:43.441730922+09:00 INFO  crosvm::crosvm::sys::unix::device_helpers] Trying to attach block device: /proc/self/fd/54
[2023-07-07T14:50:43.462055141+09:00 INFO  crosvm::crosvm::sys::unix::device_helpers] Trying to attach block device: /proc/self/fd/63
[WARN] Config entry DebugPolicy uses non-zero offset with zero size
[WARN] Config entry DebugPolicy uses non-zero offset with zero size
[INFO] pVM firmware
avb_slot_verify.c:443: ERROR: initrd_normal: Hash of data does not match digest in descriptor.
[INFO] device features: SEG_MAX | RO | BLK_SIZE | RING_EVENT_IDX | VERSION_1 | ACCESS_PLATFORM
[INFO] config: 0x201a000
[INFO] found a block device of size 50816KB
[INFO] device features: SEG_MAX | BLK_SIZE | FLUSH | DISCARD | WRITE_ZEROES | RING_EVENT_IDX | VERSION_1 | ACCESS_PLATFORM
[INFO] config: 0x2022000
[INFO] found a block device of size 10304KB
[INFO] No debug policy found.
[INFO] Starting payload...
<omitted>
07-07 05:52:01.322    69    69 I vm_payload: vm_payload: Notified host payload ready successfully
07-07 05:52:01.364    70    70 I adbd    : persist.adb.watchdog set to ''
07-07 05:52:01.364    70    70 I adbd    : persist.sys.test_harness set to ''
07-07 05:52:01.365    70    70 I adbd    : adb watchdog timeout set to 600 seconds
07-07 05:52:01.366    70    70 I adbd    : Setup mdns on port= 5555
07-07 05:52:01.366    70    70 I adbd    : adbd listening on vsock:5555
07-07 05:52:01.366    70    70 I adbd    : adbd started
#
```

The `--auto-connect` option provides you an adb-shell connection to the VM. The
shell promot (`#`) at the end of the log is for that.

## Step 5: Run tests

There are various tests that spawn guest VMs and check different aspects of the
architecture. They all can run via `atest`.

```shell
atest MicrodroidHostTestCases
atest MicrodroidTestApp
```

If you run into problems, inspect the logs produced by `atest`. Their location
is printed at the end. The `host_log_*.zip` file should contain the output of
individual commands as well as VM logs.
