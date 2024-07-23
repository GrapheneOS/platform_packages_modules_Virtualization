# Microdroid vendor modules

Starting with Android V it is possible to start a Microdroid VM with a
vendor-prodived kernel modules. This feature is part of the bigger
[device assignment](device_assignmnent.md) effort.

The vendor kernel modules should be packaged inside a `microdroid-vendor.img`
dm-verity protected partition, inside a Microdroid VM this will be mounted as
`/vendor` partition.

Currently the following features are supported:
* Kernel modules;
* init .rc scripts with basic triggers (e.g. `on early-init`);
* `ueventd.rc` file;
* `/vendor/etc/selinux/vendor_file_contexts` file.


Additionallity, starting with android15-6.6 it is possible to start a Microdroid
VM with GKI as guest kernel. This is **required** when launching a Microdroid VM with
vendor provided kernel modules.

**Note:** in Android V, the 'Microdroid vendor modules' is considered an experimental
feature to provide our partners a reference implementation that they can start
integrating with to flesh out missing pieces.
We **do not recommened** launching user-facing features that depend on using
vendor modules in a pVM.


## Integrating into a product {#build-system-integration}

You can define microdroid vendor partition using `android_filesystem` soong
module, here is an example:

```
android_filesystem {
    name: "microdroid_vendor_example_image",
    partition_name: "microdroid-vendor",
    type: "ext4",
    file_contexts: "file_contexts",
    use_avb: true,
    avb_private_key: ":microdroid_vendor_example_sign_key",
    mount_point: "vendor",
    deps: [
        "microdroid_vendor_example_ueventd",
        "microdroid_vendor_example_file_contexts",
        "microdroid_vendor_example_kernel_modules",
        "microdroid_vendor_example.rc",
    ],
}

prebuilt_etc {
    name: "microdroid_vendor_example",
    src: ":microdroid_vendor_example_image",
    relative_install_path: "avf/microdroid",
    filename: "microdroid_vendor.img",
    vendor: true,
}
```

In order to integrate the microdroid vendor partition into a product, add the
following lines to the corresponding device makefile:

```
PRODUCT_PACKAGES += microdroid_vendor_example
MICRODROID_VENDOR_IMAGE_MODULE := microdroid_vendor_example
```

**Note**: it is important that the microdroid-vendor.img is installed into
`/vendor/etc/avf/microdroid/microdroid_vendor.img` on the device.


## Launching a Microdroid VM wirth vendor partition

### Non-protected VMs

You can launch a non-protected Microdroid VM with vendor partition by adding the
`--vendor` argument to the `/apex/com.android.virt/bin/vm run-app` or
`/apex/com.android.virt/bin/vm run-microdroid` CLI commands, e.g.:

```
adb shell /apex/com.android.virt/bin/vm/run-microdroid \
  --debug full \
  --vendor /vendor/etc/avf/microdroid/microdroid_vendor.img
```

On the Android host side, the `virtmgr` will append the
`vendor_hashtree_descriptor_root_digest` property to the `/avf` node of the
guest device tree overlay. Value of this property will contain the hashtree
digest of the `microdroid_vendor.img` provided via the `--vendor` argument.

Inside the Microdroid guest VM, the `first_stage_init` will use the
`/proc/device-tree/avf/vendor_hashtree_descriptor_root_digest` to create a
`dm-verity` device on top of the `/dev/block/by-name/microdroid-vendor` block
device. The `/vendor` partition will be mounted on top of the created
`dm-verity` device.

TODO(ioffe): create drawings and add them here.


### Protected VMs

As of now, only **debuggable** Microdroid pVMs support running with the
Microdroid vendor partition, e.g.:

```
adb shell /apex/com.android.virt/bin/vm/run-microdroid \
  --debug full \
  --protected \
  --vendor /vendor/etc/avf/microdroid/microdroid_vendor.img
```

The execution flow is very similar to the non-protected case above, however
there is one important addition. The `pvmfw` binary will use the
[VM reference DT blob](#../guest/pvmfw/README.md#pvmfw-data-v1-2) passed from the
Android Bootloader (ABL), to validate the guest DT overlay passed from the host.

See [Changes in Android Bootloader](#changes-in-abl) section below for more
details.

### Reflecting microdroid vendor partition in the guest DICE chain

The microdroid vendor partition will be reflected as a separate
`Microdroid vendor` node in the Microdroid DICE chain.

TODO(ioffe): drawing of DICE chain here.

This node derivation happens in the `derive_microdroid_vendor_dice_node`, which
is executed by `first_stage_init`. The binary will write the new DICE chain into
the `/microdroid_resources/dice_chain.raw` file, which will be then read by
`microdroid_manager` to derive the final `Microdroid payload` DICE node.

TODO(ioffe): another drawing here.

## Changes in the Android Bootloader {#changes-in-abl}

In order for a Microdroid pVM with the
`/vendor/etc/avf/microdroid/microdroid_vendor.img` to successfully boot, the
ABL is required to pass the correct value of the
`/vendor/etc/avf/microdroid/microdroid_vendor.img` hashtree digest in the
`vendor_hashtree_descriptor_root_digest` property of `the /avf/reference` node.

The `MICRODROID_VENDOR_IMAGE_MODULE` make variable mentioned in the
[section above](#build-system-integration) configures build system to inject
the value of the `microdroid-vendor.img` hashtree digest into the
`com.android.build.microdroid-vendor.root_digest ` property of the footer of
the host's `vendor.img`.

The Android Bootloader can read that property when construction the
[VM reference DT blob](#../guest/pvmfw/README.md#pvmfw-data-v1-2) passed to pvmfw.

## GKI as Microdroid guest kernel

In order to enable running Microdroid with GKI as guest kernel, specify the
`PRODUCT_AVF_MICRODROID_GUEST_GKI_VERSION ` variable in a product makefile:

```
PRODUCT_AVF_MICRODROID_GUEST_GKI_VERSION := android15_66
```

Note: currently this will alter the content of the `com.android.virt` APEX by
installing the corresponding GKI image into it. In the future, the GKI image
will be installed on the `/system_ext` partition.

The following changes to the `gki_defconfig` were made to support running as
guest kernel:

```
CONFIG_VIRTIO_VSOCKETS=m
CONFIG_VIRTIO_BLK=m
CONFIG_OPEN_DICE=m
CONFIG_VCPU_STALL_DETECTOR=m
CONFIG_VIRTIO_CONSOLE=m
CONFIG_HW_RANDOM_CCTRNG=m
CONFIG_VIRTIO_PCI=m
CONFIG_VIRTIO_BALLOON=m
CONFIG_DMA_RESTRICTED_POOL=y
```

