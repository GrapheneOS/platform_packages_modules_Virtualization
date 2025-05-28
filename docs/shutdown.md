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

1. The guest agent sets the sysprop `sys.powerctl` to `"shutdown"`.

2. `init` process starts a regular Android reboot sequence with a timeout set to 2 seconds
via the sysprop `ro.build.shutdown_timeout`.

3. `init` sends `SIGTERM` to all services which actually are `microdroid_manager` and `adbd` (if the
   VM was debuggable). It actually sends the signal to the process group of where the service is a
leader of. So in case of `microdroid_manager`, the same signal is sent to `microdroid_launcher`
where the app payload runs and to `zipfuse` which also is a children of `microdroid_manager`.

4. App payload is expected to handle `SIGTERM` by setting up the signal handler if it doesn't want
   a sudden kill (which is the default behavior for `SIGTERM`) and want to do some clean-ups like
canceling the ongoing transaction, etc.

5. `microdroid_manager` waits for `microdorid_launcher` to finish; either after handling SIGTERM, or
   not.

6. `init` waits for `microdroid_manager` to finish. If it doesn't finish within the 2 seconds
   timeout, `init` forcibly kills it by sending `SIGKILL`.

7. After all userspace processes are done, `init` ask kernel to turn power down.
