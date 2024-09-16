# Getting started with device assignment

Device assignment allows a VM to have direct access to HW without host/hyp
intervention. AVF uses `vfio-platform` for device assignment, and host kernel
support is required.

This document explains how to setup and launch VM with device assignments.

## VM device assignment DTBO (a.k.a. VM DTBO)

For device assignment, a VM device assignment DTBO (a.k.a. VM DTBO) is required.
VM DTBO is a device tree overlay which describes all assignable devices
information. Information includes physical reg, IOMMU, device properties, and
dependencies.

VM DTBO allows to pass extra properties of assignable platform
devices to the VM (which can't be discovered from the HW) while keeping the VMM
device-agnostic.

When the host boots, the bootloader provides VM DTBO to both Android and pvmfw.

When a VM boots, the VMM selectively applies the DTBO based from provided
labels, describing the assigned devices.

## Prepare VM DTBO

VM DTBO should be included in the dtbo partition. It should be in its own
entry, and not together with any host OS's. See [DTB/DTBO Paritions] for
partition format.

[DTB/DTBO Paritions]: https://source.android.com/docs/core/architecture/dto/partitions

### Write VM DTS for VM DTBO

DTBO is compiled from device tree source (DTS) with `dtc` tool. [DTBO syntax]
explains basic syntax of DTS.

[DTBO syntax]: https://source.android.com/docs/core/architecture/dto/syntax

Here are details and requirements:

#### Describe assignable devices

VM DTBO should describe assignable devices and their labels.

* VM DTBO should have assignable devices in the `&{/}`, so it can be
  overlaid onto VM DT. Assignable devices should be backed by physical device.
  * We only support overlaying onto root node (i.e. `&{/}`) to prevent
    unexpected modification of VM DT.
* VM DTBO should have labels for assignable devices, so AVF can recognize
  assignable device list. Labels should point to valid 'overlayable' nodes.
  * Overlayable node is a node that would be applied to the base device tree
    when DTBO is applied.

#### Describe physical devices and physical IOMMUs

VM DTBO should describe a `/host` node which describes physical devices and
physical IOMMUs. The `/host` node only describes information for verification of
assigned devices, and wouldn't be applied to VM DT. Here are details:

* Physical IOMMU nodes
  * IOMMU nodes must have a phandle to be referenced by a physical device node.
  * IOMMU nodes must have `<android,pvmfw,token>` property. The property
    describes the IOMMU token. An IOMMU token is a hypervisor-specific `<u64>`
    which uniquely identifies a physical IOMMU. IOMMU token must be constant
    across the VM boot for provisioning by pvmfw remains valid. The token must
    be kept up-to-date across hypervisor updates.
  * IOMMU nodes should be multi-master IOMMUs. (i.e. `#iommu-cells = <1>`)
    * Other `#iommu-cells` values aren't supported for now.
    * See: [Device tree binding for IOMMUs][IOMMU]
* Physical device nodes
  * Physical device nodes must have a `<android,pvmfw,target>` property that
    references an overlayable node. The overlayable node contains the properties
    that would be included in VM DT.
  * Physical device nodes must have `<reg>` property to provide physical
    regions.
  * Physical device nodes can optionally contain `<iommus>` property. The
    property is a prop-encoded-array and contains a number of
    (iommu phandle, SID) pairs.
    * IOMMU can be shared among devices, but should use distinct SIDs. Sharing
      the same IOMMU-SID pair among multiple devices isn't supported for now.

[IOMMU]: https://www.kernel.org/doc/Documentation/devicetree/bindings/iommu/iommu.txt

#### Describe dependencies

VM DTBO may have dependencies via phandle references. When a device node is
assigned, dependencies of the node are also applied to VM DT.

When dependencies are applied, siblings or children nodes of dependencies are
ignored unless explicitly referenced.

#### VM DTBO example

Here's a simple example device tree source with four assignable devices nodes.

```dts
/dts-v1/;
/plugin/;

/ {
    // host node describes physical devices and IOMMUs, and wouldn't be applied to VM DT
    host {
        #address-cells = <0x2>;
        #size-cells = <0x1>;
        rng {
            reg = <0x0 0x12f00000 0x1000>;
            iommus = <&iommu0 0x3>;
            android,pvmfw,target = <&rng>;
        };
        light {
            reg = <0x0 0x00f00000 0x1000>, <0x0 0x00f10000 0x1000>;
            iommus = <&iommu1 0x4>, <&iommu2 0x5>;
            android,pvmfw,target = <&light>;
        };
        led {
            reg = <0x0 0x12000000 0x1000>;
            iommus = <&iommu1 0x3>;
            android,pvmfw,target = <&led>;
        };
        bus0 {
            #address-cells = <0x1>;
            #size-cells = <0x1>;
            backlight {
                reg = <0x300 0x100>;
                android,pvmfw,target = <&backlight>;
            };
        };
        iommu0: iommu0 {
            #iommu-cells = <0x1>;
            android,pvmfw,token = <0x0 0x12e40000>;
        };
        iommu1: iommu1 {
            #iommu-cells = <0x1>;
            android,pvmfw,token = <0x0 0x40000>;
        };
        iommu2: iommu2 {
            #iommu-cells = <0x1>;
            android,pvmfw,token = <0x0 0x50000>;
        };
    };
};

// Beginning of the assignable devices. Assigned devices would be applied to VM DT
&{/} {  // We only allows to overlay to root node
    rng: rng {
        compatible = "android,rng";
        android,rng,ignore-gctrl-reset;
    };
    light: light {
        compatible = "android,light";
        version = <0x1 0x2>;
    };
    led: led {
        compatible = "android,led";
        prop = <0x555>;
    };
    bus0 {
        backlight: backlight {
            compatible = "android,backlight";
            android,backlight,ignore-gctrl-reset;
        };
    };
};
```

If you compile the above with `dtc -@`, then you'll get `__symbols__` for free.
`__symbol__` has label of nodes, and it's required for the next step.

```dts
    // generated __symbols__
    __symbols__ {
        iommu0 = "/host/iommu0";
        iommu1 = "/host/iommu1";
        iommu2 = "/host/iommu2";
        rng = "/fragment@rng/__overlay__/rng";
        light = "/fragment@sensor/__overlay__/light";
        led = "/fragment@led/__overlay__/led";
        backlight = "/fragment@backlight/__overlay__/bus0/backlight";
    };
```

## Prepare AVF assignable devices XML

AVF requires assignable device information to unbind from the host device driver
and bind to VFIO driver. The information should be provided in an XML file at
`/vendor/etc/avf/assignable_devices.xml`.

Here's example.

```xml
<devices>
    <device>
        <kind>sensor</kind>
        <dtbo_label>light</dtbo_label>
        <sysfs_path>/sys/bus/platform/devices/16d00000.light</sysfs_path>
    </device>
</devices>
```

* `<kind>`: Device kind. Currently only used for debugging purposes and not used
  for device assignment.
* `<dtbo_label>`: Label in the VM DTBO (i.e. symbol in `__symbols__`). Must be
  non-empty and unique in the XML.
* `<sysfs_path>`: Sysfs path of the device in host, used to bind to the VFIO
  driver. Must be non-empty and unique in the XML.

### List support assignable devices

In order to query list of the devices that can be assigned to a pVM, run the
following command:

```bash
adb shell /apex/com.android.virt/bin/vm info
```

All supported assignable devices will be located under the "Assignable devices:"
section of the output.

## Boot with VM DTBO

Bootloader should provide VM DTBO to both Android and pvmfw.

### Provide VM DTBO index in dtbo.img

Bootloader should provide the VM DTBO index with sysprop
`ro.boot.hypervisor.vm_dtbo_idx.`. DTBO index represents DTBO location in
dtbo.img.

### Provide VM DTBO in the pvmfw config

For protected VM, bootloader must provide VM DTBO to the pvmfw. pvmfw sanitizes
incoming device tree with the VM DTBO.

For more detail about providing VM DTBO in pvmfw,
see: [pvmfw/README.md](../guest/pvmfw/README.md#configuration-data-format)


## Launch VM with device assignment

We don't support client API yet in Android V, but you can use CLI to test device
assignment. Note that host kernel support is required.

Specify `--devices ${sysfs_path}` when booting VM. The parameter can be repeated
multiple times for specifying multiple devices.

Here's an example:

```sh
adb shell /apex/com.android.virt/bin/vm run-microdroid --devices /sys/bus/platform/devices/16d00000.light
```
