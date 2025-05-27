# Shutdown

## AVF-level behavior

AVF tries to shut the VM down gracefully. The shutdown usually happens when
`VirtualMachine#stop()` method is called by the VM owner, but can happen also
when the VM owner is lost (e.g. killed).

The graceful shutdown is attempted only when the guest-side OS has a guest
agent. The guest agent is expected to be a binder service implementing
`IGuestAgent` interface. It should be registered to `IVirtualMachineService`
via the `registerGuestAgent` method. The guest agent should implement the
`shutdown` method for the guest OS.

If the guest agent doesn't exist, or the shutdown of the VM is not observed
after 5 seconds after the `shutdown` method is called, AVF forcibly kills the
VM instance by sending SIGKILL to the crosvm process.  This may cause data
corruption on the guest side, especially when it was doing some I/O.

## Microdroid-level behavior

The guest agent in Microdroid is implemented as a part of `microdroid_manager`.
Upon receiving the `shutdown` call, the shutdown sequence is performed as below:

1. The guest agent sets the sysprop `sys.shutdown.requested` to `"0"`.

2. `init` process starts a regular Android reboot sequence. Specifically, a
   reboot monitor thread is started with timeout set to 2 seconds. This timeout
value is from `ro.build.shutdown.watchdog.timeout`.

3. App payload is expected to monitor `sys.shutdown.requested` and do the
   appropriate actions before the timeout (2 sec) expires. ex: closing the
database.

4. In parallel to this, `init` issues emergency sync periodically by setting
   `/proc/sysrq-trigger` to `"s"`, and then finally do the force unmounting of
the disk by setting `/proc/sysrq-trigger`' to `"u"`.

5. As the last step, `init` ask kernel to turn power down.

6. crosvm process on the host side detects a vCPU reset and exits itself.
