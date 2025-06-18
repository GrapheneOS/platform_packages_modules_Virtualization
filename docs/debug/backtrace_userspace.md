# Dumping backtrace of userspace processes on microdroid

Like a normal adb device, you can dump backtrace of native processes running on
a Microdroid-base VM using [`debuggerd`][debuggerd]. Use `vm_shell` tool, and
then run `debuggerd`.

```sh
adb -s localhost:8000 start tombstoned
adb -s localhost:8000 debuggerd -b {PID}
```

**Note:** `tombstoned` is required for `debuggerd`, but microdroid doesn't run
`tombstoned` by default, even on debuggable VMs. As shown above, you need to
start `tombstoned` manually, before running `debuggerd`.

[debuggerd]: https://source.android.com/docs/core/tests/debug#tombstone
