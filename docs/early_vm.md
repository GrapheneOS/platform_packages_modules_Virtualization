# Early VM

Early VMs are specialized virtual machines that can run even before the `/data`
partition is mounted, unlike regular VMs. `early_virtmgr` is a binary that
serves as the interface for early VMs, functioning similarly to `virtmgr`,
which provides the [`IVirtualizationService`](../android/virtualizationservice/aidl/android/system/virtualizationservice/IVirtualizationService.aidl)
aidl interface.

To run an early VM, clients must follow these steps.

1) Early VMs need to be defined in `{partition}/etc/avf/early_vms.xml`. The
schema for this file is defined in [`early_vms.xsd`](../android/virtmgr/early_vms.xsd).

```early_vms.xml
<early_vms>
    <early_vm>
        <name>vm_demo_native_early</name>
        <cid>123</cid>
        <path>/system/bin/vm_demo_native_early</path>
    </early_vm>
</early_vms>
```

In this example, the binary `/system/bin/vm_demo_native_early` can establish a
connection with `early_virtmgr` and create a VM named `vm_demo_native_early`,
which will be assigned the static CID 123.

2) The client must have the following three or four capabilities.

* `IPC_LOCK`
* `NET_BIND_SERVICE`
* `SYS_NICE` (required if `RELEASE_AVF_ENABLE_VIRT_CPUFREQ` is enabled)
* `SYS_RESOURCES`

Typically, the client is defined as a service in an init script, where
capabilities can also be specified.

```vm_demo_native_early.rc
service vm_demo_native_early /system/bin/vm_demo_native_early
    user system
    group system virtualmachine
    capabilities IPC_LOCK NET_BIND_SERVICE SYS_RESOURCE SYS_NICE
    oneshot
    stdio_to_kmsg
    class early_hal
```

3) The client forks `early_virtmgr` instead of `virtmgr`.

The remaining steps are identical to those for regular VMs: connect to
`early_virtmgr`, obtain the `IVirtualizationService` interface, then create and
run the VM.
