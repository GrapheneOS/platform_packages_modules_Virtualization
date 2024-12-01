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
## Graphical environment (Wayland, VNC)
By installing Wayland compositor and VNC backend, you can enable graphical environment.
One of the options is `sway`, `wayvnc` and `xwayland`(if necessary).

```
sudo apt install sway wayvnc xwayland
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 sway
WAYLAND_DISPLAY=wayland-1 wayvnc 0.0.0.0 # or use port forwarding
```

And then, connect to 192.168.0.2:5900(or localhost:5900) with arbitrary VNC client.
Or, `novnc`(https://github.com/novnc/noVNC/releases). For `novnc` you need to install
`novnc`, and run `<novnc_path>/utils/novnc_proxy`, and then connect to `http://192.168.0.2:6080/vnc.html`
(or `localhost:6080` if port forwarding is enabled.)

`weston` with VNC backend might be another option, but it isn't available in
Debian package repository for bookworm.

## Hardware accelration
If the file `/sdcard/linux/virglrenderer` exists on the device, it enables VirGL for VM.
This requires enabling ANGLE for the Terminal app. (https://chromium.googlesource.com/angle/angle.git/+/HEAD/doc/DevSetupAndroid.md)
