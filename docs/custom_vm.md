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

  - Step 1) Go to the link https://ci.chromium.org/ui/p/chromeos/builders/chromiumos/ferrochrome-public-main/
    - Note: I 'searched' the ferrochrome target with builder search.
  - Step 2) Click a build number
  - Step 3) Expand steps and find `48. upload artifacts`.
  - Step 4) Click `gs upload dir`. You'll see Cloud storage with comprehensive artifacts (e.g. [Here](https://pantheon.corp.google.com/storage/browser/chromiumos-image-archive/ferrochrome-public/R126-15883.0.0) is the initial build of ferrochrome)
  - Step 5) Download `image.zip`, which contains working vmlinuz.
    - Note: DO NOT DOWNLOAD `vmlinuz.tar.xz` from the CI.
  - Step 6) Uncompress `image.zip`, and boot with `chromiumos_test_image.bin` and `boot_images/vmlinuz`.
    - Note: DO NOT USE `vmlinuz.bin`.

IMPORTANT: DO NOT USE `vmlinuz.bin` for passing to crosvm. It doesn't pick-up the correct `init` process (picks `/init` instead of `/sbin/init`, and `cfg80211` keeps crashing (i.e. no network)


#### Build ChromiumOS for VM

First, check out source code from the ChromiumOS and Chromium projects.

* Checking out ChromiumOS: https://www.chromium.org/chromium-os/developer-library/guides/development/developer-guide/
* Checking out Chromium: https://g3doc.corp.google.com/chrome/chromeos/system_services_team/dev_instructions/g3doc/setup_checkout.md?cl=head

Important: When you are at the step “Set up gclient args” in the Chromium checkout instruction, configure .gclient as follows.

```
$ cat ~/chromium/.gclient
solutions = [
  {
    "name": "src",
    "url": "https://chromium.googlesource.com/chromium/src.git",
    "managed": False,
    "custom_deps": {},
    "custom_vars": {
      "checkout_src_internal": True,
    },
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

You need two outputs:

* ChromiumOS disk image: ~/chromiumos/src/build/images/ferrochrome/latest/chromiumos_test_image.bin
* The kernel: ~/chromiumos/src/build/images/ferrochrome/latest/boot_images/vmlinuz

### Create a guest VM configuration

Push the kernel and the main image to the Android device.

```
$ adb push  ~/chromiumos/src/build/images/ferrochrome/latest/chromiumos_test_image.bin /data/local/tmp/
$ adb push ~/chromiumos/out/build/ferrochrome/boot/vmlinuz /data/local/tmp/kernel
```

Create a VM config file as below.

```
$ cat > vm_config.json; adb push vm_config.json /data/local/tmp
{
    "name": "cros",
    "kernel": "/data/local/tmp/kernel",
    "disks": [
        {
            "image": "/data/local/tmp/chromiumos_test_image.bin",
            "partitions": [],
            "writable": true
        }
    ],
    "params": "root=/dev/vda3 rootwait noinitrd ro enforcing=0 cros_debug cros_secure",
    "protected": false,
    "cpu_topology": "match_host",
    "platform_version": "~1.0",
    "memory_mib" : 8096
}
```

### Running the VM

First, enable the `VmLauncherApp` app. This needs to be done only once. In the
future, this step won't be necesssary.

```
$ adb root
$ adb shell pm enable com.android.virtualization.vmlauncher/.MainActivity
$ adb unroot
```

If virt apex is Google-signed, you need to enable the app and grant the
permission to the app.
```
$ adb root
$ adb shell pm enable com.google.android.virtualization.vmlauncher/com.android.virtualization.vmlauncher.MainActivity
$ adb shell pm grant com.google.android.virtualization.vmlauncher android.permission.USE_CUSTOM_VIRTUAL_MACHINE
$ adb unroot
```
Then execute the below to set up the network. In the future, this step won't be necessary.

```
$ cat > setup_network.sh; adb push setup_network.sh /data/local/tmp
#!/system/bin/sh

set -e

TAP_IFACE=crosvm_tap
TAP_ADDR=192.168.1.1
TAP_NET=192.168.1.0

function setup_network() {
  local WAN_IFACE=$(ip route get 8.8.8.8 2> /dev/null | awk -- '{printf $5}')
  if [ "${WAN_IFACE}" == "" ]; then
    echo "No network. Connect to a WiFi network and start again"
    return 1
  fi

  if ip link show ${TAP_IFACE} &> /dev/null ; then
    echo "TAP interface ${TAP_IFACE} already exists"
    return 1
  fi

  ip tuntap add mode tap group virtualmachine vnet_hdr ${TAP_IFACE}
  ip addr add ${TAP_ADDR}/24 dev ${TAP_IFACE}
  ip link set ${TAP_IFACE} up
  ip rule flush
  ip rule add from all lookup ${WAN_IFACE}
  ip route add ${TAP_NET}/24 dev ${TAP_IFACE} table ${WAN_IFACE}
  sysctl net.ipv4.ip_forward=1
  iptables -t filter -F
  iptables -t nat -A POSTROUTING -s ${TAP_NET}/24 -j MASQUERADE
}

function setup_if_necessary() {
  if [ "$(getprop ro.crosvm.network.setup.done)" == 1 ]; then
    return
  fi
  echo "Setting up..."
  check_privilege
  setup_network
  setenforce 0
  chmod 666 /dev/tun
  setprop ro.crosvm.network.setup.done 1
}

function check_privilege() {
  if [ "$(id -u)" -ne 0 ]; then
    echo "Run 'adb root' first"
    return 1
  fi
}

setup_if_necessary
^D

adb root; adb shell /data/local/tmp/setup_network.sh
```

Then, finally tap the VmLauncherApp app from the launcher UI. You will see
Ferrochrome booting!

If it doesn’t work well, try

```
$ adb shell pm clear com.android.virtualization.vmlauncher
```

### Inside guest OS (for ChromiumOS only)

Go to the network setting and configure as below.

* IP: 192.168.1.2 (other addresses in the 192.168.1.0/24 subnet also works)
* netmask: 255.255.255.0
* gateway: 192.168.1.1
* DNS: 8.8.8.8 (or any DNS server you know)

These settings are persistent; stored in chromiumos_test_image.bin. So you
don’t have to repeat this next time.`
