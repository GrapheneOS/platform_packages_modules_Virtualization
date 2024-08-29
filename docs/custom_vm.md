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

### Running Debian
1. Download an ARM64 image from https://cloud.debian.org/images/cloud/ (We tested nocloud image)

2. Resize the image
```shell
truncate -s 20G debian.img
virt-resize --expand /dev/sda1 <download_image_file> debian.img
```

3. Copy the image file
```shell
tar cfS debian.img.tar debian.img
adb push debian.img.tar /data/local/tmp/
adb shell tar xf /data/local/tmp/debian.img.tar -C /data/local/tmp/
adb shell rm /data/local/tmp/debian.img.tar
adb shell chmod a+w /data/local/tmp/debian.img
rm debian.img.tar
```

Note: we tar and untar to keep the image file sparse.

4. Make the VM config file
```shell
cat > vm_config.json <<EOF
{
    "name": "debian",
    "disks": [
        {
            "image": "/data/local/tmp/debian.img",
            "partitions": [],
            "writable": true
        }
    ],
    "protected": false,
    "cpu_topology": "match_host",
    "platform_version": "~1.0",
    "memory_mib": 8096,
    "debuggable": true,
    "console_out": true,
    "connect_console": true,
    "console_input_device": "ttyS0",
    "network": true,
    "input": {
        "touchscreen": true,
        "keyboard": true,
        "mouse": true,
        "trackpad": true,
        "switches": true
    },
    "audio": {
        "speaker": true,
         "microphone": true
    },
    "gpu": {
        "backend": "virglrenderer",
        "context_types": ["virgl2"]
    },
    "display": {
        "refresh_rate": "30"
    }
}
EOF
adb push vm_config.json /data/local/tmp/
```

5. Launch VmLauncherApp(the detail will be explain below)

6. For console, we can refer to `Debugging` section below. (id: root)

7. For graphical shell, you need to install xfce(for now, only xfce is tested)
```
apt install task-xfce-desktop
dpkg --configure -a (if necessary)
systemctl set-default graphical.target

# need non-root user for graphical shell
adduser linux
# optional
adduser linux sudo
reboot
```

## Graphical VMs

To run OSes with graphics support, simply
`packages/modules/Virtualization/tests/ferrochrome/ferrochrome.sh --forever`.
It prepares and launches the ChromiumOS, which is the only officially supported
guest payload. We will be adding more OSes in the future.

If you want to do so by yourself, follow the instruction below.

### Prepare a guest image

As of today (April 2024), ChromiumOS is the only officially supported guest
payload. We will be adding more OSes in the future.

#### Download ChromiumOS from build server

Download
https://storage.googleapis.com/chromiumos-image-archive/ferrochrome-public/R128-15926.0.0/chromiumos_test_image.tar.xz.
The above will download ferrochrome test image with version `R128-15926.0.0`.

To download latest version, use following code.

```sh
URL=https://storage.googleapis.com/chromiumos-image-archive/ferrochrome-public
LATEST_VERSION=$(curl -s ${URL}/LATEST-main)
curl -O ${URL}/${LATEST_VERSION}/chromiumos_test_image.tar.xz
```

To navigate build server artifacts,
[install gsutil](https://cloud.google.com/storage/docs/gsutil_install).
`gs://chromiumos-image-archive/ferrochrome-public` is the top level directory for ferrochrome build.

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
    "protected": false,
    "cpu_topology": "match_host",
    "platform_version": "~1.0",
    "memory_mib": 8096,
    "debuggable": true,
    "console_out": true,
    "connect_console": true,
    "console_input_device": "hvc0",
    "network": true,
    "input": {
        "touchscreen": true,
        "keyboard": true,
        "mouse": true,
        "trackpad": true,
        "switches": true
    },
    "audio": {
        "speaker": true,
        "microphone": true
    },
    "gpu": {
        "backend": "virglrenderer",
        "context_types": ["virgl2"]
    },
    "display": {
        "scale": "0.77",
        "refresh_rate": "30"
    }
}
```

### Running the VM

1. Grant permission to the `VmLauncherApp` if the virt apex is Google-signed.
    ```shell
    $ adb shell su root pm grant com.google.android.virtualization.vmlauncher android.permission.USE_CUSTOM_VIRTUAL_MACHINE
    ```

2. Ensure your device is connected to the Internet.

3. Launch the app with adb.
    ```shell
    $ adb shell su root am start-activity -a android.virtualization.VM_LAUNCHER
    ```

If it doesn’t work well, try

```
$ adb shell pm clear com.android.virtualization.vmlauncher
# or
$ adb shell pm clear com.google.android.virtualization.vmlauncher
```

### Debugging

To open the serial console (interactive terminal):
```shell
$ adb shell -t /apex/com.android.virt/bin/vm console
```

To see console logs only, check
`/data/user/${current_user_id}/com{,.google}.android.virtualization.vmlauncher/files/${vm_name}.log`

You can monitor console out as follows

```shell
$ adb shell 'su root tail +0 -F /data/user/$(am get-current-user)/com{,.google}.android.virtualization.vmlauncher/files/${vm_name}.log'
```

For ChromiumOS, you can enter to the console via SSH connection. Check your IP
address of ChromiumOS VM from the ethernet network setting page and follow
commands below.

```shell
$ adb kill-server ; adb start-server
$ adb shell nc -s localhost -L -p 9222 nc ${CHROMIUMOS_IPV4_ADDR} 22 # This command won't be terminated.
$ adb forward tcp:9222 tcp:9222
$ ssh -oProxyCommand=none -o UserKnownHostsFile=/dev/null root@localhost -p 9222
```

For ChromiumOS, you would need to login after enthering its console.
The user ID and the password is `root` and `test0000` respectively.
