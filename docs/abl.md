# Android Bootloader (ABL)

[ABL](https://source.android.com/docs/core/architecture/bootloader) is not a component of AVF, but
it plays a crucial role in loading the necessary AVF components and initializing them in a correct
way. This doc explains the responsibilities of ABL from the perspective of AVF.

## pVM firmware (pvmfw)

ABL is responsible for the followings:

* locating pvmfw binary from the pvmfw partition,
* verifying it as part of the [verified
  boot](https://source.android.com/docs/security/features/verifiedboot) process,
* loading it into memory, and
* describing the region where pvmfw is loaded using DT and passing it to hypervisor.

See [ABL Support](../guest/pvmfw/README.md#android-bootloader-abl_support) for more detail.

ABL is also responsible for constructing the pvmfw configuration data. The data consists of the
following info:

* DICE chain (also known as BCC Handover)
* DTBO describing [debug policy](debug/README.md#debug-policy) (if available)
* DTBO describing [assignable devices](device_assignment.md) (if available)
* Reference DT carrying extra information that needs to be passed to the guest VM, e.g.:
    * Hashtree digest of the
      [microdroid-vendor.img](microdroid_vendor_modules.md#changes-in-abl).

See [Configuration Data](../guest/pvmfw/README.md#configuration-data) for more detail.

## Android

ABL is responsible for setting the following bootconfigs describing the status and capabilities of
the hypervisor.

* `androidboot.hypervisor.version`: free-form description of the hypervisor
* `androidboot.hypervisor.vm.supported`: whether traditional VMs (i.e.  non-protected VMS) are
  supported or not
* `androidboot.hypervisor.protected_vm.supported`: whether protected VMs are supported or not

Thee bootconfigs are converted into system properties by the init process.

See
[HypervisorProperties.prop](https://android.googlesource.com/platform/system/libsysprop/+/refs/heads/main/srcs/android/sysprop/HypervisorProperties.sysprop)
for more detail.










