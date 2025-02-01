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

# Terminal app
## Run GUI apps
Execute `source enable_display` and then click Display button above to enable display feature.
And then, go back to the terminal, and run GUI apps.

## Hardware acceleration
If the file `/sdcard/linux/virglrenderer` exists on the device, it enables VirGL for VM.
This requires enabling ANGLE for the Terminal app. (https://chromium.googlesource.com/angle/angle.git/+/HEAD/doc/DevSetupAndroid.md)
