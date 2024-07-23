# Android Virtualization Framework API

These Java APIs allow an app to configure and run a Virtual Machine running
[Microdroid](../build/microdroid/README.md) and execute native code from the app (the
payload) within it.

There is more information on AVF [here](../README.md). To see how to package the
payload code that is to run inside a VM, and the native API available to it, see
the [VM Payload API](../libs/libvm_payload/README.md)

The API classes are all in the
[`android.system.virtualmachine`](src/android/system/virtualmachine) package.

All of these APIs were introduced in API level 34 (Android 14). The classes may
not exist in devices running an earlier version.

Note that they are all `@SystemApi` and require the restricted
`android.permission.MANAGE_VIRTUAL_MACHINE` permission, so they are not
available to third party apps. In Android 14 the permission was available only to
privileged apps; in Android 15 it is available to all preinstalled apps. On both
versions it can also be granted to other apps via `adb shell pm grant` for
development purposes.


## Detecting AVF Support

The simplest way to detect whether a device has support for AVF is to retrieve
an instance of the
[`VirtualMachineManager`](src/android/system/virtualmachine/VirtualMachineManager.java)
class; if the result is not `null` then the device has support. You can then
find out whether protected, non-protected VMs, or both are supported using the
`getCapabilities()` method. Note that this code requires API level 34 or higher:

```Java
VirtualMachineManager vmm = context.getSystemService(VirtualMachineManager.class);
if (vmm == null) {
    // AVF is not supported.
} else {
    // AVF is supported.
    int capabilities = vmm.getCapabilities();
    if ((capabilties & CAPABILITY_PROTECTED_VM) != 0) {
        // Protected VMs supported.
    }
    if ((capabilties & CAPABILITY_NON_PROTECTED_VM) != 0) {
        // Non-Protected VMs supported.
    }
}
```

An alternative for detecting AVF support is to query support for the
`android.software.virtualization_framework` system feature. This method will
work on any API level, and return false if it is below 34:

```Java
if (getPackageManager().hasSystemFeature(PackageManager.FEATURE_VIRTUALIZATION_FRAMEWORK)) {
    // AVF is supported.
}
```

You can also express a dependency on this system feature in your app's manifest
with a
[`<uses-feature>`](https://developer.android.com/guide/topics/manifest/uses-feature-element)
element.


## Starting a VM

Once you have an instance of the
[`VirtualMachineManager`](src/android/system/virtualmachine/VirtualMachineManager.java),
a VM can be started by:
- Specifying the desired VM configuration, using a
  [`VirtualMachineConfig`](src/android/system/virtualmachine/VirtualMachineConfig.java)
  builder;
- Creating a new
  [`VirtualMachine`](src/android/system/virtualmachine/VirtualMachine.java)
  instance (or retrieving an existing one);
- Registering to retrieve events from the VM by providing a
  [`VirtualMachineCallback`](src/android/system/virtualmachine/VirtualMachineCallback.java)
  (optional, but recommended);
- Running the VM.

A minimal example might look like this:

```Java
VirtualMachineConfig config =
        new VirtualMachineConfig.Builder(this)
            .setProtectedVm(true)
            .setPayloadBinaryName("my_payload.so")
            .build();

VirtualMachine vm = vmm.getOrCreate("my vm", config);

vm.setCallback(executor, new VirtualMachineCallback() {...});

vm.run();
```

Here we are running a protected VM, which will execute the code in the
`my_payload.so` file included in your APK.

Information about the VM, including its configuration, is stored in files in
your app's private data directory. The file names are based on the VM name you
supply. So once an instance of a VM has been created it can be retrieved by name
even if the app is restarted or the device is rebooted. Directly inspecting or
modifying these files is not recommended.

The `getOrCreate()` call will retrieve an existing VM instance if it exists (in
which case the `config` parameter is ignored), or create a new one
otherwise. There are also separate `get()` and `create()` methods.

The `run()` method is asynchronous; it returns successfully once the VM is
starting. You can find out when the VM is ready, or if it fails, via your
`VirtualMachineCallback` implementation.

## VM Configuration

There are other things that you can specify as part of the
[`VirtualMachineConfig`](src/android/system/virtualmachine/VirtualMachineConfig.java):
- Whether the VM should be debuggable. A debuggable VM is not secure, but it
  does allow access to logs from inside the VM, which can be useful for
  troubleshooting.
- How much memory should be available to the VM. (This is an upper bound;
  typically memory is allocated to the VM as it is needed until the limit is
  reached - but there is some overhead proportional to the maximum size.)
- How many virtual CPUs the VM has.
- How much encrypted storage the VM has.
- The path to the installed APK containing the code to run as the VM
  payload. (Normally you don't need this; the APK path is determined from the
  context passed to the config builder.)

## VM Life-cycle

To find out the progress of the Virtual Machine once it is started you should
implement the methods defined by
[`VirtualMachineCallback`](src/android/system/virtualmachine/VirtualMachineCallback.java). These
are called when the following events happen:
- `onPayloadStarted()`: The VM payload is about to be run.
- `onPayloadReady()`: The VM payload is running and ready to accept
  connections. (This notification is triggered by the payload code, using the
  [`AVmPayload_notifyPayloadReady()`](../libs/libvm_payload/include/vm_payload.h)
  function.)
- `onPayloadFinished()`: The VM payload has exited normally. The exit code of
  the VM (the value returned by [`AVmPayload_main()`](../libs/libvm_payload/README.md))
  is supplied as a parameter.
- `onError()`: The VM failed; something went wrong. An error code and
  human-readable message are provided which may help diagnosing the problem.
- `onStopped()`: The VM is no longer running. This is the final notification
  from any VM run, whether or not it was successful. You can run the VM again
  when you want to. A reason code indicating why the VM stopped is supplied as a
  parameter.

You can also query the status of a VM at any point by calling `getStatus()` on
the `VirtualMachine` object. This will return one of the following values:
- `STATUS_STOPPED`: The VM is not running - either it has not yet been started,
  or it stopped after running.
- `STATUS_RUNNING`: The VM is running. Your payload inside the VM may not be
  running, since the VM may be in the process of starting or stopping.
- `STATUS_DELETED`: The VM has been deleted, e.g. by calling the `delete()`
  method on
  [`VirtualMachineManager`](src/android/system/virtualmachine/VirtualMachineManager.java). This
  is irreversible - once a VM is in this state it will never leave it.

Some methods on
[`VirtualMachine`](src/android/system/virtualmachine/VirtualMachine.java) can
only be called when the VM status is `STATUS_RUNNING` (e.g. `stop()`), and some
can only be called when the it is `STATUS_STOPPED` (e.g. `run()`).

## VM Identity and Secrets

Every VM has a 32-byte secret unique to it, which is not available to the
host. We refer to this as the VM identity.  The secret, and thus the identity,
doesn’t normally change if the same VM is stopped and started, even after a
reboot.

In Android 14 the secret is derived, using the [Open Profile for
DICE](https://pigweed.googlesource.com/open-dice/+/refs/heads/main/docs/android.md),
from:
- A device-specific randomly generated value;
- The complete system image;
- A per-instance salt;
- The code running in the VM, including the bootloader, kernel, Microdroid and
  payload;
- Significant VM configuration options, e.g.  whether the VM is debuggable.

Any change to any of these will mean a different secret is generated.  So while
an attacker could start a similar VM with maliciously altered code, that VM will
not have access to the same secret. An attempt to start an existing VM instance
which doesn't derive the same secret will fail.

However, this also means that if the payload code changes - for example, your
app is updated - then this also changes the identity. An existing VM instance
will no longer be runnable, and you will have to delete it and create a new
instance with a new secret.

The payload code is not given direct access to the VM secret, but an API is
provided to allow deterministically deriving further secrets from it,
e.g. encryption or signing keys. See
[`AVmPayload_getVmInstanceSecret()`](../libs/libvm_payload/include/vm_payload.h).

Some VM configuration changes are allowed that don’t affect the identity -
e.g. changing the number of CPUs or the amount of memory allocated to the
VM. This can be done using the `setConfig()` method on
[`VirtualMachine`](src/android/system/virtualmachine/VirtualMachine.java).

Deleting a VM (using the `delete()` method on
[`VirtualMachineManager`](src/android/system/virtualmachine/VirtualMachineManager.java))
and recreating it will generate a new salt, so the new VM will have a different
secret, even if it is otherwise identical.

## Communicating with a VM

Once the VM payload has successfully started you will probably want to establish
communication between it and your app.

Only the app that started a VM can connect to it. The VM can accept connections
from the app, but cannot initiate connections to other VMs or other processes in
the host Android.

### Vsock

The simplest form of communication is using a socket running over the
[vsock](https://man7.org/linux/man-pages/man7/vsock.7.html) protocol.

We suggest that the VM payload should create a listening socket (using the
standard socket API) and then trigger the `onPayloadReady()` callback; the app
can then connect to the socket. This helps to avoid a race condition where the
app tries to connect before the VM is listening, necessitating a retry
mechanism.

In the payload this might look like this:

```C++
#include "vm_payload.h"

extern "C" int AVmPayload_main() {
  int fd = socket(AF_VSOCK, SOCK_STREAM, 0);
  // bind, listen
  AVmPayload_notifyPayloadReady();
  // accept, read/write, ...
}
```

And, in the app, like this:

```Java
void onPayloadReady(VirtualMachine vm) {
  ParcelFileDescriptor pfd = vm.connectVsock(port);
  // ...
}
```

Vsock is useful for simple communication, or transferring of bulk data. For a
richer RPC style of communication we suggest using Binder.

### Binder

The use of AIDL interfaces between the VM and app is supported via Binder RPC,
which transmits messages over an underlying vsock socket.

Note that Binder RPC has some limitations compared to the kernel Binder used in
Android - for example file descriptors can't be sent. It also isn't possible to
send a kernel Binder interface over Binder RPC, or vice versa.

There is a payload API to allow an AIDL interface to be served over a specific
vsock port, and the VirtualMachine class provides a way to connect to the VM and
retrieve an instance of the interface.

The payload code to serve a hypothetical `IPayload` interface might look like
this:

```C++
class PayloadImpl : public BnPayload { ... };


extern "C" int AVmPayload_main() {
  auto service = ndk::SharedRefBase::make<PayloadImpl>();
  auto callback = [](void*) {
    AVmPayload_notifyPayloadReady();
  };
  AVmPayload_runVsockRpcServer(service->asBinder().get(),
    port, callback, nullptr);
}

```

And then the app code to connect to it could look like this:

```Java
void onPayloadReady(VirtualMachine vm) {
  IPayload payload =
    Payload.Stub.asInterface(vm.connectToVsockServer(port));
  // ...
}
```

## Stopping a VM

You can stop a VM abruptly by calling the `stop()` method on the
[`VirtualMachine`](src/android/system/virtualmachine/VirtualMachine.java)
instance. This is equivalent to turning off the power; the VM gets no
opportunity to clean up at all. Any unwritten data might be lost.

A better strategy might be to wait for the VM to exit cleanly (e.g. waiting for
the `onStopped()` callback).

Then you can arrange for your VM payload code to exit when it has finished its
task (by returning from [`AVmPayload_main()`](../libs/libvm_payload/README.md), or
calling `exit()`). Alternatively you could exit when you receive a request to do
so from the app, e.g. via binder.

When the VM payload does this you will receive an `onPayloadFinished()`
callback, if you have installed a
[`VirtualMachineCallback`](src/android/system/virtualmachine/VirtualMachineCallback.java),
which includes the payload's exit code.

Use of `stop()` should be reserved as a recovery mechanism - for example if the
VM has not stopped within a reasonable time (a few seconds, say) after being
requested to.

The status of a VM will be `STATUS_STOPPED` if your `onStopped()` callback is
invoked, or after a successful call to `stop()`. Note that your `onStopped()`
will be called on the VM even if it ended as a result of a call to `stop()`.

# Encrypted Storage

When configuring a VM you can specify that it should have access to an encrypted
storage filesystem of up to a specified size, using the
`setEncryptedStorageBytes()` method on a
[`VirtualMachineConfig`](src/android/system/virtualmachine/VirtualMachineConfig.java)
builder.

Inside the VM this storage is mounted at a path that can be retrieved via the
[`AVmPayload_getEncryptedStoragePath()`](../libs/libvm_payload/include/vm_payload.h)
function. The VM can create sub-directories and read and write files here. Any
data written is persisted and should be available next time the VM is run. (An
automatic sync is done when the payload exits normally.)

Outside the VM the storage is persisted as a file in the app’s private data
directory. The data is encrypted using a key derived from the VM secret, which
is not made available outside the VM.

So an attacker should not be able to decrypt the data; however, a sufficiently
powerful attacker could delete it, wholly or partially roll it back to an
earlier version, or modify it, corrupting the data.

For more info see [README](https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/Virtualization/libs/framework-virtualization/README.md)

# Transferring a VM

It is possible to make a copy of a VM instance. This can be used to transfer a
VM from one app to another, which can be useful in some circumstances.

This should only be done while the VM is stopped. The first step is to call
`toDescriptor()` on the
[`VirtualMachine`](src/android/system/virtualmachine/VirtualMachine.java)
instance, which returns a
[`VirtualMachineDescriptor`](src/android/system/virtualmachine/VirtualMachineDescriptor.java)
object. This object internally contains open file descriptors to the files that
hold the VM's state (its instance data, configuration, and encrypted storage).

A `VirtualMachineDescriptor` is
[`Parcelable`](https://developer.android.com/reference/android/os/Parcelable),
so it can be passed to another app via a Binder call.  Any app with a
`VirtualMachineDescriptor` can pass it, along with a new VM name, to the
`importFromDescriptor()` method on
[`VirtualMachineManager`](src/android/system/virtualmachine/VirtualMachineManager.java). This
is equivalent to calling `create()` with the same name and configuration, except
that the new VM is the same instance as the original, with the same VM secret,
and has access to a copy of the original's encrypted storage.

Once the transfer has been completed it would be reasonable to delete the
original VM, using the `delete()` method on `VirtualMachineManager`.





