# Demo for serving a service in a VM

You can implement a service in a VM, and let client in the Android can use it
as if it's in the Android. To do so, implement IAccessor.

IAccessor allows AIDL service in a VM can be accessed via servicemanager.
To do so, VM owners should also provide IAccessor implementation. servicemanager
will connect to the IAccessor and get the binder of the service in a VM with it.

com.android.virt.accessor_demo apex contains the minimum setup for IAccessor as
follows:
  - accessor_demo: Sample implementation of IAccessor, which is expected to
      launch VM and returns the Vsock connection of service in the VM.
  - AccessorVmApp: Sample app that conatins VM payload. Provides the actual
      implementation of service in a VM.

## Build

You need to do envsetup.sh
```shell
m com.android.virt.accessor_demo
```

## Install (requires userdebug build)

For very first install,

```shell
adb remount -R || adb wait-for-device  # Remount to push apex to /system_ext
adb root && adb remount                # Ensure it's rebooted.
adb push $ANDROID_PRODUCT_OUT/system_ext/apex/com.android.virt.accessor_demo.apex /system_ext/apex
adb reboot && adb wait-for-device      # Ensure that newly pushed apex at /system_ext is installed
```

Once it's installed, you can simply use `adb install` for update.

```shell
adb install $ANDROID_PRODUCT_OUT/system_ext/com.android.virt.accessor_demo.apex
adb reboot
```
