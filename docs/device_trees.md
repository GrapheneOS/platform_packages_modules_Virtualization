# Device Trees in AVF

This document aims to provide a centralized overview of the way the Android
Virtualization Framework (AVF) composes and validates the device tree (DT)
received by protected guest kernels, such as [Microdroid].

[Microdroid]: ../guest/microdroid/README.md

## Context

As of Android 15, AVF only supports protected virtual machines (pVMs) on
AArch64. On this architecture, the Linux kernel and many other embedded projects
have adopted the [device tree format][dtspec] as the way to describe the
platform to the software. This includes so-called "[platform devices]" (which are
non-discoverable MMIO-based devices), CPUs (number, characteristics, ...),
memory (address and size), and more.

With virtualization, it is common for the virtual machine manager (VMM, e.g.
crosvm or QEMU), typically a host userspace process, to generate the DT as it
configures the virtual platform. In the case of AVF, the threat model prevents
the guest from trusting the host and therefore the DT must be validated by a
trusted entity. To avoid adding extra logic in the highly-privileged hypervisor,
AVF relies on [pvmfw], a small piece of code that runs in the context of the
guest (but before the guest kernel), loaded by the hypervisor, which validates
the untrusted device tree. If any anomaly is detected, pvmfw aborts the boot of
the guest. As a result, the guest kernel can trust the DT it receives.

The DT sanitized by pvmfw is received by guests following the [Linux boot
protocol][booting.txt] and includes both virtual and physical devices, which are
hardly distinguishable from the guest's perspective (although the context could
provide information helping to identify the nature of the device e.g. a
virtio-blk device is likely to be virtual while a platform accelerator would be
physical). The guest is not expected to treat physical devices differently from
virtual devices and this distinction is therefore not relevant.

```
┌────────┐               ┌───────┐ valid              ┌───────┐
│ crosvm ├──{input DT}──►│ pvmfw ├───────{guest DT}──►│ guest │
└────────┘               └───┬───┘                    └───────┘
                             │   invalid
                             └───────────► SYSTEM RESET
```

[dtspec]: https://www.devicetree.org/specifications
[platform devices]: https://docs.kernel.org/driver-api/driver-model/platform.html
[pvmfw]: ../guest/pvmfw/README.md
[booting.txt]: https://www.kernel.org/doc/Documentation/arm64/booting.txt

## Device Tree Generation (Host-side)

crosvm describes the virtual platform to the guest by generating a DT
enumerating the memory region, virtual CPUs, virtual devices, and other
properties (e.g. ramdisk, cmdline, ...). For physical devices (assigned using
VFIO), it generates simple nodes describing the fundamental properties it
configures for the devices i.e. `<reg>`, `<interrupts>`, `<iommus>`
(respectively referring to IPA ranges, vIRQs, and pvIOMMUs).

It is possible for the caller of crosvm to pass more DT properties or nodes to
the guest by providing device tree overlays (DTBO) to crosvm. These overlays get
applied after the DT describing the configured platform has been generated, the
final result getting passed to the guest.

For physical devices, crosvm supports applying a "filtered" subset of the DTBO
received, where subnodes are only kept if they have a label corresponding to an
assigned VFIO device. This allows the caller to always pass the same overlay,
irrespective of which physical devices are being assigned, greatly simplifying
the logic of the caller. This makes it possible for crosvm to support complex
nodes for physical devices without including device-specific logic as any extra
property (e.g. `<compatible>`) will be passed through the overlay and added to
the final DT in a generic way. This _vm DTBO_ is read from an AVB-verified
partition (see `ro.boot.hypervisor.vm_dtbo_idx`).

Otherwise, if the `filter` option is not used, crosvm applies the overlay fully.
This can be used to supplement the guest DT with nodes and properties which are
not tied to particular assigned physical devices or emulated virtual devices. In
particular, `virtualizationservice` currently makes use of it to pass
AVF-specific properties.

```
            ┌─►{DTBO,filter}─┐
┌─────────┐ │                │  ┌────────┐
│ virtmgr ├─┼────►{DTBO}─────┼─►│ crosvm ├───►{guest DT}───► ...
└─────────┘ │                │  └────────┘
            └─►{VFIO sysfs}──┘
```

## Device Tree Sanitization

pvmfw intercepts the boot sequence of the guest and locates the DT generated by
the VMM through the VMM-guest ABI. A design goal of pvmfw is to have as little
side-effect as possible on the guest so that the VMM can keep the illusion that
it configured and booted the guest directly and the guest does not need to rely
or expect pvmfw to have performed any noticeable work (a noteworthy exception
being the memory region describing the [DICE chain]). As a result, both VMM and
guest can mostly use the same logic between protected and non-protected VMs
(where pvmfw does not run) and keep the simpler VMM-guest execution model they
are used to. In the context of pvmfw and DT validation, the final DT passed by
crosvm to the guest is typically referred to as the _input DT_.

```
┌────────┐                  ┌───────┐                  ┌───────┐
│ crosvm ├───►{input DT}───►│ pvmfw │───►{guest DT}───►│ guest │
└────────┘                  └───────┘                  └───────┘
                              ▲   ▲
   ┌─────┐  ┌─►{VM DTBO}──────┘   │
   │ ABL ├──┤                     │
   └─────┘  └─►{ref. DT}──────────┘
```

[DICE chain]: ../guest/pvmfw/README.md#virtual-platform-dice-chain-handover

### Virtual Platform

The DT sanitization policy in pvmfw matches the virtual platform defined by
crosvm and its implementation is therefore tightly coupled with it (this is one
reason why AVF expects pvmfw and the VMM to be updated in sync). It covers
fundamental properties of the platform (e.g.  location of main memory,
properties of CPUs, layout of the interrupt controller, ...) and the properties
of (sometimes optional) virtual devices supported by crosvm and used by AVF
guests.

### Physical Devices

To support device assignment, pvmfw needs to be able to validate physical
platform-specific device properties. To achieve this in a platform-agnostic way,
pvmfw receives a DT overlay (called the _VM DTBO_) from the Android Bootloader
(ABL), containing a description of all the assignable devices. By detecting
which devices have been assigned using platform-specific reserved DT labels, it
can validate the properties of the physical devices through [generic logic].
pvmfw also verifies with the hypervisor that the guest addresses from the DT
have been properly mapped to the expected physical addresses of the devices; see
[_Getting started with device assignment_][da.md].

Note that, as pvmfw runs within the context of an individual pVM, it cannot
detect abuses by the host of device assignment across guests (e.g.
simultaneously assigning the same device to multiple guests), and it is the
responsibility of the hypervisor to enforce this isolation. AVF also relies on
the hypervisor to clear the state of the device on donation and (most
importantly) on return to the host so that pvmfw does not need to access the
assigned devices.

[generic logic]: ../guest/pvmfw/src/device_assignment.rs
[da.md]: ../docs/device_assignment.md

### Extra Properties (Security-Sensitive)

Some AVF use-cases require passing platform-specific inputs to protected guests.
If these are security-sensitive, they must also be validated before being used
by the guest. In most cases, the DT property is platform-agnostic (and supported
by the generic guest) but its value is platform-specific. The _reference DT_ is
an [input of pvmfw][pvmfw-config] (received from the loader) and used to
validate DT entries which are:

- security-sensitive: the host should not be able to tamper with these values
- not confidential: the property is visible to the host (as it generates it)
- Same across VMs: the property (if present) must be same across all instances
- possibly optional: pvmfw does not abort the boot if the entry is missing

[pvmfw-config]: ../guest/pvmfw/README.md#configuration-data-format

### Extra Properties (Host-Generated)

Finally, to allow the host to generate values that vary between guests (and
which therefore can't be described using one the previous mechanisms), pvmfw
treats the subtree of the input DT at path `/avf/untrusted` differently: it only
performs minimal sanitization on it, allowing the host to pass arbitrary,
unsanitized DT entries. Therefore, this subtree must be used with extra
validation by guests e.g. only accessed by path (where the name, "`untrusted`",
acts as a reminder), with no assumptions about the presence or correctness of
nodes or properties, without expecting properties to be well-formed, ...

In particular, pvmfw prevents other nodes from linking to this subtree
(`<phandle>` is rejected) and limits the risk of guests unexpectedly parsing it
other than by path (`<compatible>` is also rejected) but guests must not support
non-standard ways of binding against nodes by property as they would then be
vulnerable to attacks from a malicious host.

### Implementation details

DT sanitization is currently implemented in pvmfw by parsing the input DT into
temporary data structures and pruning a built-in device tree (called the
_platform DT_; see [platform.dts]) accordingly. For device assignment, it prunes
the received VM DTBO to only keep the devices that have actually been assigned
(as the overlay contains all assignable devices of the platform).

[platform.dts]: ../guest/pvmfw/platform.dts

## DT for guests

### AVF-specific properties and nodes

For Microdroid and other AVF guests, some special DT entries are defined:

- the `/chosen/avf,new-instance` flag, set when pvmfw triggered the generation
  of a new set of CDIs (see DICE) _i.e._ the pVM instance was booted for the
  first time. This should be used by the next stages to synchronise the
  generation of new CDIs and detect a malicious host attempting to force only
  one stage to do so. This property becomes obsolete (and might not be set) when
  [deferred rollback protection] is used by the guest kernel;

- the `/chosen/avf,strict-boot` flag, always set for protected VMs and can be
  used by guests to enable extra validation;

- the `/avf/untrusted/defer-rollback-protection` flag controls [deferred
  rollback protection] on devices and for guests which support it;

- the host-allocated `/avf/untrusted/instance-id` is used to assign a unique
  identifier to the VM instance & is used for differentiating VM secrets as well
  as by guest OS to index external storage such as Secretkeeper.

[deferred rollback protection]: ../docs/updatable_vm.md#deferring-rollback-protection