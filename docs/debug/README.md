# Debugging protected VMs

AVF is largely about protected VMs. This in turn means that anything that is
happening inside the VM cannot be observed from outside of the VM. But as a
developer, you need to be able to look into it when there’s an error in your
VM. To satisfy such contradictory needs, AVF allows you to start a protected VM
in a debuggable mode and provides a bunch of debugging mechanisms you can use
to better understand the behavior of the VM and diagnose issues.

Note: running a protected VM in a debuggable mode introduces many loopholes
which can be used to nullify the protection provided by the hypervisor.
Therefore, the debugable mode should never be used in production.

## Enable debugging

The following sections describe the two ways debugging can be enabled.

### Debug level

Debug level is a per-VM property which indicates how debuggable the VM is.
There currently are two levels defined: NONE and FULL. NONE means that the VM
is not debuggable at all, and FULL means that [all the debugging
features](#debugging-features) are supported.

Debug level is by default NONE. You can set it to FULL either via a Java API
call in your app or via a command line argument `--debug` as follows:

```java
VirtualMachineConfig.Builder.setDebugLevel(DEBUG_LEVEL_FULL);
```

or

```shell
adb shell /apex/com.android.virt/bin/vm run-microdroid --debug full
```

or

```shell
m vm_shell
vm_shell start-microdroid --auto-connect -- --protected --debug full
```

Note: `--debug full` is the default option when omitted. You need to explicitly
use `--debug none` to set the debug level to NONE.

### Debug policy

Debug policy is a per-device property which forcibly enables selected debugging
features, even for the VMs with debug level NONE.

The main purpose of debug policy is in-field debugging by the platform
developers (device makers, SoC vendors, etc.) To understand it, let’s imagine
that you have an application of pVM. It’s configured as debug level NONE
because you finished the development and the team-level testing. However, you
get a bug report from your QA team or from beta testers. To fix the bug, you
should be able to look into the pVM but you do not want to change the source
code to make the VM debuggable and rebuild the entire software, because that
may hurt the reproducibility of the bug.

Note: Not every devices is guaranteed to support debug policy. It is up to the
device manufacturer to implement this in their bootloader. Google Pixel
devices for example support this after Pixel 7 and 7 Pro. Pixel 6 and 6 Pro
don't support debug policy.

In the Pixel phones supporting debug policy, it is provisioned by installing a
device tree overlay like below to the Pixel-specific partition `dpm`.

```
/ {
    fragment@avf {
        target-path = "/";

        __overlay__ {
            avf {
                guest {
                    common {
                        log = <1>; // Enable kernel log and logcat
                        ramdump = <1>; // Enable ramdump
                    }
                    microdroid {
                        adb = <1>; // Enable ADB connection
                    }
                }
            };
        };
    };
}; /* end of avf */
```

To not enable a specific debugging feature, set the corresponding property
value to other than `<1>`, or delete the property.

As a reference, in Pixel phones, debug policy is loaded as below:

1. Bootloader loads it from the `dpm` partition and verifies it.
1. Bootloader appends the loaded debug policy as the [configuration
   data](../../guest/pvmfw/README.md#configuration-data) of the pvmfw.
1. When a pVM is started, pvmfw [overlays][apply_debug_policy] the debug policy to the baseline
   device tree from crosvm.
1. OS payload (e.g. Microdroid) [reads][read_debug_policy] the device tree and enables specific
   debugging feature accordingly.

**Note**: Bootloader MUST NOT load debug policy when the bootloader is in LOCKED state.

[apply_debug_policy]: https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/Virtualization/pvmfw/src/fdt.rs;drc=0d52747770baa14d44c0779b5505095b4251f2e9;l=790
[read_debug_policy]: https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/Virtualization/microdroid_manager/src/main.rs;drc=65c9f1f0eee4375535f2025584646a0dbb0ea25c;l=834

## Debugging features

AVF currently supports the following debugging features:

* ADB connection (only for Microdroid)
* Capturing console output
* Capturing logcat output (only for Microdroid)
* [Capturing kernel ramdump](ramdump.md) (only for Microdroid)
* Capturing userspace crash dump (only for Microdroid)
* [Attaching GDB to the kernel](gdb_kernel.md)
* [Attaching GDB to the userspace process](gdb_userspace.md) (only for Microdroid)
* [Tracing hypervisor events](tracing.md)
