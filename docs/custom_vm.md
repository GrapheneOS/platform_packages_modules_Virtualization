# Custom VM

## Headless VMs

If your VM is headless (i.e. console in/out is the primary way of interacting
with it), you can spawn it by passing a JSON config file to the
VirtualizationService via the `vm` tool on a rooted AVF-enabled device. If your
device is attached over ADB, you can run:

```shell
cat > vm_config.json <<EOF
{
  "kernel": "/data/local/tmp/kernel",
  "initrd": "/data/local/tmp/ramdisk",
  "params": "rdinit=/bin/init"
}
EOF
adb root
adb push <kernel> /data/local/tmp/kernel
adb push <ramdisk> /data/local/tmp/ramdisk
adb push vm_config.json /data/local/tmp/vm_config.json
adb shell "/apex/com.android.virt/bin/vm run /data/local/tmp/vm_config.json"
```

The `vm` command also has other subcommands for debugging; run
`/apex/com.android.virt/bin/vm help` for details.

### Running Debian with u-boot
1. Prepare u-boot binary from `u-boot_crosvm_aarch64` in https://ci.android.com/builds/branches/aosp_u-boot-mainline/grid
or build it by https://source.android.com/docs/devices/cuttlefish/bootloader-dev#develop-bootloader
2. Prepare Debian image from https://cloud.debian.org/images/cloud/ (We tested nocloud image)
3. Copy `u-boot.bin`, Debian image file(like `debian-12-nocloud-arm64.raw`) and `vm_config.json` to `/data/local/tmp`
```shell
cat > vm_config.json <<EOF
{
    "name": "debian",
    "bootloader": "/data/local/tmp/u-boot.bin",
    "disks": [
        {
            "image": "/data/local/tmp/debian-12-nocloud-arm64.raw",
            "partitions": [],
            "writable": true
        }
    ],
    "protected": false,
    "cpu_topology": "match_host",
    "platform_version": "~1.0",
    "memory_mib" : 8096
}
EOF
adb push `u-boot.bin` /data/local/tmp
adb push `debian-12-nocloud-arm64.raw` /data/local/tmp
adb push vm_config.json /data/local/tmp/vm_config.json
```
4. Launch VmLauncherApp(the detail will be explain below)

## Graphical VMs

To run OSes with graphics support, follow the instruction below.

### Prepare a guest image

As of today (April 2024), ChromiumOS is the only officially supported guest
payload. We will be adding more OSes in the future.

#### Download from build server

Here's the link the comprehensive artifacts
https://pantheon.corp.google.com/storage/browser/chromiumos-image-archive/ferrochrome-public

Pick a build, download, and untar `chromiumos_test_image.tar.xz`. We'll boot with `chromiumos_test_image.bin` in it.

To find the latest green build, check following:
https://pantheon.corp.google.com/storage/browser/_details/chromiumos-image-archive/ferrochrome-public/LATEST-main

#### Build ChromiumOS for VM

First, check out source code from the ChromiumOS and Chromium projects.

* Checking out Chromium: https://www.chromium.org/developers/how-tos/get-the-code/
* Checking out ChromiumOS: https://www.chromium.org/chromium-os/developer-library/guides/development/developer-guide/

Important: When you are at the step “Set up gclient args” in the Chromium checkout instruction, configure .gclient as follows.

```
$ cat ~/chromium/.gclient
solutions = [
  {
    "name": "src",
    "url": "https://chromium.googlesource.com/chromium/src.git",
    "managed": False,
    "custom_deps": {},
    "custom_vars": {},
  },
]
target_os = ['chromeos']
```

In this doc, it is assumed that ChromiumOS is checked out at `~/chromiumos` and
Chromium is at `~/chromium`. If you downloaded to different places, you can
create symlinks.

Then enter into the cros sdk.

```
$ cd ~/chromiumos
$ cros_sdk --chrome-root=$(readlink -f ~/chromium)
```

Now you are in the cros sdk. `(cr)` below means that the commands should be
executed inside the sdk.

First, choose the target board. `ferrochrome` is the name of the virtual board
for AVF-compatible VM.

```
(cr) setup_board --board=ferrochrome
```

Then, tell the cros sdk that you want to build chrome (the browser) from the
local checkout and also with your local modifications instead of prebuilts.

```
(cr) CHROME_ORIGIN=LOCAL_SOURCE
(cr) ACCEPT_LICENSES='*'
(cr) cros workon -b ferrochrome start \
chromeos-base/chromeos-chrome \
chromeos-base/chrome-icu
```

Optionally, if you have touched the kernel source code (which is under
~/chromiumos/src/third_party/kernel/v5.15), you have to tell the cros sdk that
you want it also to be built from the modified source code, not from the
official HEAD.

```
(cr) cros workon -b ferrochrome start chromeos-kernel-5_15
```

Finally, build individual packages, and build the disk image out of the packages.

```
(cr) cros build-packages --board=ferrochrome --chromium --accept-licenses='*'
(cr) cros build-image --board=ferrochrome --no-enable-rootfs-verification test
```

This takes some time. When the build is done, exit from the sdk.

Note: If build-packages doesn’t seem to include your local changes, try
invoking emerge directly:

```
(cr) emerge-ferrochrome -av chromeos-base/chromeos-chrome
```

Don’t forget to call `build-image` afterwards.

You need ChromiumOS disk image: ~/chromiumos/src/build/images/ferrochrome/latest/chromiumos_test_image.bin

### Create a guest VM configuration

Push the kernel and the main image to the Android device.

```
$ adb push  ~/chromiumos/src/build/images/ferrochrome/latest/chromiumos_test_image.bin /data/local/tmp/
```

Create a VM config file as below.

```
$ cat > vm_config.json; adb push vm_config.json /data/local/tmp
{
    "name": "cros",
    "disks": [
        {
            "image": "/data/local/tmp/chromiumos_test_image.bin",
            "partitions": [],
            "writable": true
        }
    ],
    "gpu": {
        "backend": "virglrenderer",
        "context_types": ["virgl2"]
    },
    "params": "root=/dev/vda3 rootwait noinitrd ro enforcing=0 cros_debug cros_secure",
    "protected": false,
    "cpu_topology": "match_host",
    "platform_version": "~1.0",
    "memory_mib" : 8096,
    "console_input_device": "ttyS0"
}
```

### Running the VM

First, enable the `VmLauncherApp` app. This needs to be done only once. In the
future, this step won't be necesssary.

```
$ adb root
$ adb shell pm enable com.android.virtualization.vmlauncher/.MainActivityAlias
$ adb unroot
```

If virt apex is Google-signed, you need to enable the app and grant the
permission to the app.
```
$ adb root
$ adb shell pm enable com.google.android.virtualization.vmlauncher/com.android.virtualization.vmlauncher.MainActivityAlias
$ adb shell pm grant com.google.android.virtualization.vmlauncher android.permission.USE_CUSTOM_VIRTUAL_MACHINE
$ adb unroot
```

Second, ensure your device is connected to the Internet.

Finally, tap the VmLauncherApp app from the launcher UI. You will see
Ferrochrome booting!

If it doesn’t work well, try

```
$ adb shell pm clear com.android.virtualization.vmlauncher
# or
$ adb shell pm clear com.google.android.virtualization.vmlauncher
```

### Inside guest OS (for ChromiumOS only)

Go to the network setting and configure as below.

* IP: 192.168.1.2 (other addresses in the 192.168.1.0/24 subnet also works)
* netmask: 255.255.255.0
* gateway: 192.168.1.1
* DNS: 8.8.8.8 (or any DNS server you know)

These settings are persistent; stored in chromiumos_test_image.bin. So you
don’t have to repeat this next time.

### Debugging

To open the serial console (interactive terminal):
```shell
$ adb shell -t /apex/com.android.virt/bin/vm console
```

To see console logs only, check
`/data/data/com.android.virtualization.vmlauncher/files/console.log`
Or
`/data/data/com.google.android.virtualization.vmlauncher/files/console.log`

```shell
$ adb shell su root tail +0 -F /data/data/com{,.google}.android.virtualization.vmlauncher/files/console.log
```

For ChromiumOS, you can ssh-in. Use following commands after network setup.

```shell
$ adb kill-server ; adb start-server; adb forward tcp:9222 tcp:9222
$ ssh -oProxyCommand=none -o UserKnownHostsFile=/dev/null root@localhost -p 9222
```
