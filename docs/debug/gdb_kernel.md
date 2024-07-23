# Debugging guest kernels with gdb

Note: this feature is only available on android14-5.15 and newer host kernels.

Starting with Android U it is possible to attach a gdb to the guest kernel, when
starting a debuggable and non-protected guest VM.

You can do this by passing `--gdb <port>` argument to the `vm run`, `vm run-app`
and `vm run-microdroid` commands. The `crosvm` will start the gdb server on the
provided port. It will wait for the gdb client to connect to it before
proceeding with the VM boot.

Here is an example invocation:

```shell
adb forward tcp:3456 tcp:3456
adb shell /apex/com.android.virt/bin/vm run-microdroid --gdb 3456
```

Then in another shell:

```shell
gdb vmlinux
(gdb) target remote :3456
(gdb) hbreak start_kernel
(gdb) c
```

The [kernel documentation](
https://www.kernel.org/doc/html/latest/dev-tools/gdb-kernel-debugging.html) has
some general techniques on how to debug kernel with gdb.

## Obtaining vmlinux for Microdroid kernels

If you are debugging Microdroid kernel that you have built [locally](
../../build/microdroid/kernel/README.md), then look for `out/dist/vmlinux` in your
kernel repository.

If you are debugging Microdroid kernel bundled with the `com.android.virt` APEX,
then you need to obtain the build ID of this kernel. You can do this by
checking the prebuilt-info.txt file in the
`packages/modules/Virtualization/build/microdroid/kernel/arm64` or
`packages/modules/Virtualization/build/microdroid/kernel/x86_64` directories.

Using that build ID you can download the vmlinux from the build server via:
https://ci.android.com/builds/submitted/${BUILD_ID}/kernel_microdroid_aarch64/latest/vmlinux
