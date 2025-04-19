# Hypervisor & guest tracing

## Hypervisor tracing

Starting with android14-5.15 kernel it is possible to get traces from the hypervisor.

### User space interface

The user space hypervisor tracing interface is located either at /sys/kernel/tracing/hyp or at
/sys/kernel/debug/tracing/hyp. On the Android phones it will usually be /sys/kernel/tracing/hyp,
while on QEMU it will be /sys/kernel/debug/tracing/hyp.

The user space interface is very similar to the ftrace user space interface, however there are some
differences, e.g.:

* Only boot clock is supported, and there is no way for user space to change the tracing_clock.
* Hypervisor tracing periodically polls the data from the hypervisor, this is different from the
  regular ftrace instance which pushes the events into the ring buffer.
* Resetting ring buffers (by clearing the trace file) is only supported when there are no active
  readers. If the trace file is cleared while there are active readers, then the ring buffers will
  be cleared after the last reader disconnects.
* Changing the size of the ring buffer while the tracing session is active is also not supported.

Note: the list above is not exhaustive.

### Perfetto integration

[Perfetto](https://perfetto.dev/docs/) is an open-source stack for performance instrumentation and
trace analysis widely used in  Android. Perfetto supports capturing and visualizing hypervisor
traces.

#### Capturing hypervisor traces on Android

Consider first familiarizing yourself with Perfetto documentation for recording traces on Android:
https://perfetto.dev/docs/quickstart/android-tracing.

The [record_android_trace](
https://cs.android.com/android/platform/superproject/+/master:external/perfetto/tools/record_android_trace)
script supports a shortcut to capture all hypervisor events that are  known to Perfetto:

```shell
external/perfetto/tools/record_android_trace hyp -t 15s -b 32mb -o /tmp/hyp.pftrace
```

Alternatively you can use full trace config to capture hypervisor. Example usage:

```shell
cat<<EOF>config.pbtx
duration_ms: 10000

buffers: {
    size_kb: 8960
    fill_policy: DISCARD
}

data_sources: {
    config {
        name: "linux.ftrace"
        ftrace_config {
            instance_name: "hyp"
            ftrace_events: "hyp/hyp_enter"
            ftrace_events: "hyp/hyp_exit"
        }
    }
}
EOF

./record_android_trace -c config.pbtx -o trace_file.perfetto-trace
```

If you have an Android tree checked out, then record_android_trace helper script can be located at
${REPO_ROOT}/external/perfetto/tools/record_android_traces. Otherwise, you can download the script
by following steps outlined in the [Perfetto docs](
https://perfetto.dev/docs/quickstart/android-tracing#recording-a-trace-through-the-cmdline)

#### Capturing hypervisor traces on QEMU

Perfetto supports capturing traces on Linux: https://perfetto.dev/docs/quickstart/linux-tracing.
However, since pKVM hypervisor is only supported on arm64, you will need to cross-compile Perfetto
binaries for linux-arm64 (unless you have an arm64 workstation).

1. Checkout Perfetto repository: https://perfetto.dev/docs/contributing/getting-started
2. Follow https://perfetto.dev/docs/contributing/build-instructions#cross-compiling-for-linux-arm-64
  to compile Perfetto binaries for arm64 architecture.
3. Copy the tracebox binary to QEMU
4. Run `tracebox` binary on QEMU to capture traces, it's interface is very similar to the
`record_android_trace` binary. E.g. to capture all hypervisor events run:
```shell
tracebox -t 15s -b 32mb hyp
```

### Analysing traces using SQL

On top of visualisation, Perfetto also provides a SQL interface to analyse traces. More
documentation is available at https://perfetto.dev/docs/quickstart/trace-analysis and
https://perfetto.dev/docs/analysis/trace-processor.

Hypervisor events can be queried via `pkvm_hypervisor_events` SQL view. You can load that view by
calling `SELECT IMPORT("pkvm.hypervisor");`, e.g.:

```sql
SELECT IMPORT("pkvm.hypervisor");
SELECT * FROM pkvm_hypervisor_events limit 5;
```

Below are some SQL queries that might be useful when analysing hypervisor traces.

**What is the longest time CPU spent in hypervisor, grouped by the reason to enter hypervisor**
```sql
SELECT IMPORT("pkvm.hypervisor");

SELECT
  cpu,
  reason,
  ts,
  dur
FROM pkvm_hypervisor_events
JOIN (
  SELECT
    MAX(dur) as dur2,
    cpu as cpu2,
    reason as reason2
  FROM pkvm_hypervisor_events
  GROUP BY 2, 3) AS sc
ON
  cpu = sc.cpu2
  AND dur = sc.dur2
  AND (reason = sc.reason2 OR (reason IS NULL AND sc.reason2 IS NULL))
ORDER BY dur desc;
```

**What are the 10 longest times CPU spent in hypervisor because of host_mem_abort**
```sql
SELECT
  hyp.dur as dur,
  hyp.ts as ts,
  EXTRACT_ARG(slices.arg_set_id, 'esr') as esr,
  EXTRACT_ARG(slices.arg_set_id, 'addr') as addr
FROM pkvm_hypervisor_events as hyp
JOIN slices
ON hyp.slice_id = slices.id
WHERE hyp.reason = 'host_mem_abort'
ORDER BY dur desc
LIMIT 10;
```

## Microdroid VM tracing

IMPORTANT: Tracing is only supported for debuggable Microdroid VMs.

### Capturing trace in Microdroid

Starting with Android U, Microdroid contains Perfetto tracing binaries, which makes it possible to
capture traces inside Microdroid VM using Perfetto stack. The commands used to capture traces on
Android should work for Microdroid VM as well, with a difference that Perfetto's tracing binaries
are not enabled in Microdroid by default, so you need to manually start them by setting
`persist.traced.enable` system property to `1`.

Here is a quick example on how trace Microdroid VM:

1. First start your VM. For this example we are going to use
`adb shell /apex/com.android.virt/bin/vm run-microdroid`.

2. Set up an adb connection with the running VM:
```shell
adb forward tcp:9876 vsock:${CID}:5555
adb connect localhost:9876
adb -s localhost:9876 root
```
Where `${CID}` corresponds to the running Microdroid VM that you want to establish adb connection
with. List of running VMs can be obtained by running `adb shell /apex/com.android.virt/bin/vm list`.
Alternatively you can use `vm_shell` utility to connect to a running VM, i.e.: `vm_shell connect`.

3. Start Perfetto daemons and capture trace
```shell
adb -s localhost:9876 shell setprop persist.traced.enable 1
${ANDROID_BULD_TOP}/external/perfetto/tools/record_android_trace \
  -s localhost:9876 \
  -o /tmp/microdroid-trace-file.pftrace \
  -t 10s \
  -b 32mb \
  sched/sched_switch task/task_newtask sched/sched_process_exit
```

If you don't have Android repo checked out, then you can download the record_android_trace script by
following the following [instructions](
https://perfetto.dev/docs/quickstart/android-tracing#recording-a-trace-through-the-cmdline)

More documentation on Perfetto's tracing on Android is available here:
https://perfetto.dev/docs/quickstart/android-tracing

### Capturing Microdroid boot trace

TODO(b/271412868): Stay tuned, more docs are coming soon!
