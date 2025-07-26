# Dumping VMs with bugreport

On `adb bugreport` (or `dumpsys android.system.virtualizationservice`), `virtualizationservice` dumps basic information of running VMs.

```
vsoc_x86_64:/ # dumpsys android.system.virtualizationservice
Running 1 VMs:
VM CID: 2048
    name: VmRunApp
    temporary_directory: /data/misc/virtualizationservice/2048
    requester_uid: 2000
    requester_debug_pid: 26649
    VM dump begin
    (VM dump contents...)
    VM dump end
```

## Implement Detailed VM Dump

In addition to basic information, guest OSes can implement `IGuestAgent`'s `startDumpVsockServer` method to dump detailed internal state of VMs.

```aidl
int startDumpVsockServer(in String[] args);
```

When `startDumpVsockServer` is called, the guest agent should:

1.  **Start a vsock server**: Open a vsock server and listen for an incoming connection.
2.  **Dump VM state**: Upon a client connection (from `virtualizationservice`), the guest OS must write dump to the connected vsock stream.
3.  **Timeout**: The entire dumping process (including writing to the vsock) should complete within **5 seconds**. `virtualizationservice` may otherwise consider it a timeout.

`virtualizationservice` then embeds this detailed dump directly into the `adb bugreport` output.

## Microdroid's VM Dump Implementation

The guest agent in `microdroid_manager` implements basic dump for Microdroid VMs, focusing on memory and process-related statistics. The dump includes the contents of the following files:

* `/proc/meminfo`
* `/proc/pressure/*`
* `/sys/fs/cgroup/*/memory.*` (excluding `memory.reclaim`)
* `/proc/*/smaps_rollup`
* `/proc/pagetypeinfo`
* `/proc/vmstat`
* `/proc/zoneinfo`
* `/proc/*/smaps` (**debuggable VMs only**)
