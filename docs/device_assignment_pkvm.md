# Device Assignment Integration Guide (pKVM)

This document outlines the requirements and steps to integrate device assignment support for Android Virtualization Framework (AVF) using pKVM (Protected KVM). Device assignment allows a protected VM (pVM) to have direct, exclusive access to a hardware device.

## Overview

In the pKVM model, the host kernel is untrusted by the guest pVM. Therefore, the hypervisor (EL2) must mediate device assignment to ensure:
1.  **Isolation:** The device is isolated from the host and other VMs using the IOMMU, and the MMIO is not accessible from host.
2.  **Sanitization:** The device is reset before being assigned to the guest to prevent data leakage from the host and vice versa.

## Prerequisites

These are some of the main prerequisites to be able to use device assignment.

### Hardware
*   **Architecture:** `arm64` only.
*   **IOMMU:** The hardware must have an IOMMU capable of isolating the specific platform devices intended for assignment.
*   **Platform Devices:** Currently, only platform devices are supported. PCI device assignment is not yet supported in pKVM.
* **Device programming** Device MMIO base address and size must be page aligned.
* **Coherency** Device must be cache coherent (In Android16-6.12)

### Software
*   **Kernel:** Android 16 (kernel 6.12) or higher.
*   **Hypervisor:** pKVM enabled.
*   **VM Type:** Only Protected VMs (pVMs) currently support device assignment.

## Integration Steps

To enable device assignment, changes are required across the stack: kernel, device tree, and userspace.

### 1. Kernel & Hypervisor Configuration

#### Hypervisor Reset Handler
Unlike traditional virtualization, pKVM considers the host malicious. The host cannot be trusted to reset the device safely. Therefore, the **hypervisor** must be able to reset the device.

*   You must implement a reset handler for your specific device within the pKVM hypervisor (EL2) modules.
*   This handler is registered using `device_register_reset` (internal pKVM API).
*   **Requirement:** The reset function MUST strictly clear any internal state that could leak data across device transitions.

**Note:** That is different from the kernel VFIO-platform reset, that is not required in the pKVM model and can be disabled with kernel command line `vfio-platform.reset_required=0`

#### Host Device Tree
The hypervisor needs to know which devices are "assignable" at boot time. This is defined in the host device tree.

Add a `pkvm_assignable_devices` node to your host device tree:

```dts
    pkvm_assignable_devices {
        compatible = "pkvm,device-assignment";
        // List of <phandle group_id> pairs
        devices = <&device_node_1 group_id_1>, ..., <&device_node_N group_id_N>;
    };
```

*   `&device_node_X`: The phandle to the platform device node in the DT.
*   `group_id`: An integer identifier. Devices with the same `group_id` are considered a group (similar to IOMMU groups) and must be assigned together.

#### IOMMU Support
Starting with kernel 6.12, GKI guests use the `pkvm-pviommu` driver.
*   Ensure your hypervisor implements the necessary IOMMU driver support.
*   References:
    *   [pviommu Documentation](https://android.googlesource.com/kernel/common/+/refs/heads/android16-6.12/Documentation/virt/kvm/arm/pviommu.rst)
    *   [Device Tree Bindings](https://android.googlesource.com/kernel/common/+/refs/heads/android16-6.12/Documentation/devicetree/bindings/iommu/pkvm%2Cpviommu.yaml)

### 2. Guest Configuration

#### Guest Device Tree (DTBO)
The guest needs to know about the device it is receiving. This is handled via a Device Tree Overlay (DTBO).
*   See [device_assignment.md](./device_assignment.md) for the detailed schema and requirements for the VM DTBO.
*   **Note:** `crosvm` uses this to generate the final guest DT, and `pvmfw` uses it to verify the configuration.

#### Vendor Modules
The guest VM likely needs a specific driver to drive the hardware.
*   These drivers should be packaged in a vendor partition.
*   See [microdroid_vendor_modules.md](./microdroid_vendor_modules.md) for instructions on creating and loading vendor modules in Microdroid.

### 3. Android Userspace

#### Device Enumeration
Android userspace needs to know which devices are available for assignment.
*   Define the assignable devices in the `assignable_devices.xml` configuration file.
*   See [device_assignment.md](./device_assignment.md) for the XML format.

## Architecture & Design Details

### High-Level Flow

1.  **Request:** A request is made to start a VM with a specific device (using sysfs node)
2.  **Binding:** `crosvm` (the VMM) interacts with the host kernel's `vfio-platform` driver to bind the device.
3.  **KVM Assignment:** `crosvm` uses the KVM-VFIO device to request the hypervisor to assign the device to the VM context.
    *   See [KVM VFIO Documentation](https://android.googlesource.com/kernel/common/+/refs/heads/android16-6.12/Documentation/virt/kvm/devices/vfio.rst).
4.  **Reset:** The hypervisor invokes the registered reset handler for the device to ensure it is clean.
5.  **Verification (pvmfw):** Before the guest code runs, `pvmfw` (Protected VM Firmware) validates the device assignment against the trusted VM DTBO.
    *   `pvmfw` uses [hypercalls](https://android.googlesource.com/kernel/common/+/refs/heads/android16-6.12/Documentation/virt/kvm/arm/hypercalls.rst) to query the hypervisor.
6.  **Access:** The VM boots, loads the vendor driver, and accesses the device directly.
