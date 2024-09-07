# Protected Virtual Machine Firmware

In the context of the [Android Virtualization Framework][AVF], a hypervisor
(_e.g._ [pKVM]) enforces full memory isolation between its virtual machines
(VMs) and the host.  As a result, the host is only allowed to access memory that
has been explicitly shared back by a VM. Such _protected VMs_ (“pVMs”) are
therefore able to manipulate secrets without being at risk of an attacker
stealing them by compromising the Android host.

As pVMs are started dynamically by a _virtual machine manager_ (“VMM”) running
as a host process and as pVMs must not trust the host (see [_Why
AVF?_][why-avf]), the virtual machine it configures can't be trusted either.
Furthermore, even though the isolation mentioned above allows pVMs to protect
their secrets from the host, it does not help with provisioning them during
boot. In particular, the threat model would prohibit the host from ever having
access to those secrets, preventing the VMM from passing them to the pVM.

To address these concerns the hypervisor securely loads the pVM firmware
(“pvmfw”) in the pVM from a protected memory region (this prevents the host or
any pVM from tampering with it), setting it as the entry point of the virtual
machine. As a result, pvmfw becomes the very first code that gets executed in
the pVM, allowing it to validate the environment and abort the boot sequence if
necessary. This process takes place whenever the VMM places a VM in protected
mode and can’t be prevented by the host.

Given the threat model, pvmfw is not allowed to trust the devices or device
layout provided by the virtual platform it is running on as those are configured
by the VMM. Instead, it performs all the necessary checks to ensure that the pVM
was set up as expected. For functional purposes, the interface with the
hypervisor, although trusted, is also validated.

Once it has been determined that the platform can be trusted, pvmfw derives
unique secrets for the guest through the [_DICE Chain_][android-dice] (see
[Open Profile for DICE][open-dice]) that can be used to prove the identity of
the pVM to local and remote actors. If any operation or check fails, or in case
of a missing prerequisite, pvmfw will abort the boot process of the pVM,
effectively preventing non-compliant pVMs and/or guests from running.
Otherwise, it hands over the pVM to the guest kernel by jumping to its first
instruction, similarly to a bootloader.

pvmfw currently only supports AArch64.

[AVF]: https://source.android.com/docs/core/virtualization
[why-avf]: https://source.android.com/docs/core/virtualization/whyavf
[android-dice]: https://pigweed.googlesource.com/open-dice/+/refs/heads/main/docs/android.md
[pKVM]: https://source.android.com/docs/core/virtualization/architecture#hypervisor
[open-dice]: https://pigweed.googlesource.com/open-dice/+/refs/heads/main/docs/specification.md

## Integration

### pvmfw Loading

When running pKVM, the physical memory from which the hypervisor loads pvmfw
into guest address space is not initially populated by the hypervisor itself.
Instead, it receives a pre-loaded memory region from a trusted pvmfw loader and
only then becomes responsible for protecting it. As a result, the hypervisor is
kept generic (beyond AVF) and small as it is not expected (nor necessary) for it
to know how to interpret or obtain the content of that region.

#### Android Bootloader (ABL) Support

Starting in Android T, the `PRODUCT_BUILD_PVMFW_IMAGE` build variable controls
the generation of `pvmfw.img`, a new [ABL partition][ABL-part] containing the
pvmfw binary (sometimes called "`pvmfw.bin`") and following the internal format
of the [`boot`][boot-img] partition, intended to be verified and loaded by ABL
on AVF-compatible devices.

Once ABL has verified the `pvmfw.img` chained static partition, the contained
[`boot.img` header][boot-img] may be used to obtain the size of the `pvmfw.bin`
image (recorded in the `kernel_size` field), as it already does for the kernel
itself. In accordance with the header format, the `kernel_size` bytes of the
partition following the header will be the `pvmfw.bin` image.

Note that when it gets executed in the context of a pVM, `pvmfw` expects to have
been loaded at 4KiB-aligned intermediate physical address (IPA) so if ABL loads
the `pvmfw.bin` image without respecting this alignment, it is the
responsibility of the hypervisor to either reject the image or copy it into
guest address space with the right alignment.

To support pKVM, ABL is expected to describe the region using a reserved memory
device tree node where both address and size have been properly aligned to the
page size used by the hypervisor. This single region must include both the pvmfw
binary image and its configuration data (see below). For example, the following
node describes a region of size `0x40000` at address `0x80000000`:
```
reserved-memory {
    ...
    pkvm_guest_firmware {
        compatible = "linux,pkvm-guest-firmware-memory";
        reg = <0x0 0x80000000 0x40000>;
        no-map;
    }
}
```

[ABL-part]: https://source.android.com/docs/core/architecture/bootloader/partitions
[boot-img]: https://source.android.com/docs/core/architecture/bootloader/boot-image-header

### Configuration Data

As part of the process of loading pvmfw, the loader (typically the Android
Bootloader, "ABL") is expected to pass device-specific pvmfw configuration data
by appending it to the pvmfw binary and including it in the region passed to the
hypervisor. As a result, the hypervisor will give the same protection to this
data as it does to pvmfw and will transparently load it in guest memory, making
it available to pvmfw at runtime. This enables pvmfw to be kept device-agnostic,
simplifying its adoption and distribution as a centralized signed binary, while
also being able to support device-specific details.

The configuration data will be read by pvmfw at the next 4KiB boundary from the
end of its loaded binary. Even if the pvmfw is position-independent, it will be
expected for it to also have been loaded at a 4-KiB boundary. As a result, the
location of the configuration data is implicitly passed to pvmfw and known to it
at build time.

#### Configuration Data Format

The configuration data is described using the following [header]:

```
+===============================+
|          pvmfw.bin            |
+~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~+
|  (Padding to 4KiB alignment)  |
+===============================+ <-- HEAD
|      Magic (= 0x666d7670)     |
+-------------------------------+
|           Version             |
+-------------------------------+
|   Total Size = (TAIL - HEAD)  |
+-------------------------------+
|            Flags              |
+-------------------------------+
|           [Entry 0]           |
|  offset = (FIRST - HEAD)      |
|  size = (FIRST_END - FIRST)   |
+-------------------------------+
|           [Entry 1]           |
|  offset = (SECOND - HEAD)     |
|  size = (SECOND_END - SECOND) |
+-------------------------------+
|           [Entry 2]           | <-- Entry 2 is present since version 1.1
|  offset = (THIRD - HEAD)      |
|  size = (THIRD_END - THIRD)   |
+-------------------------------+
|           [Entry 3]           | <-- Entry 3 is present since version 1.2
|  offset = (FOURTH - HEAD)     |
|  size = (FOURTH_END - FOURTH) |
+-------------------------------+
|              ...              |
+-------------------------------+
|           [Entry n]           |
+~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~+
| (Padding to 8-byte alignment) |
+===============================+ <-- FIRST
|   {First blob: DICE chain}    |
+~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~+ <-- FIRST_END
| (Padding to 8-byte alignment) |
+===============================+ <-- SECOND
|       {Second blob: DP}       |
+~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~+ <-- SECOND_END
| (Padding to 8-byte alignment) |
+===============================+ <-- THIRD
|     {Third blob: VM DTBO}     |
+~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~+ <-- THIRD_END
| (Padding to 8-byte alignment) |
+===============================+ <-- FOURTH
| {Fourth blob: VM reference DT}|
+~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~+ <-- FOURTH_END
| (Padding to 8-byte alignment) |
+===============================+
|              ...              |
+===============================+ <-- TAIL
```

Where the version number is encoded using a "`major.minor`" as follows

```
((major << 16) | (minor & 0xffff))
```

and defines the format of the header (which may change between major versions),
its size and, in particular, the expected number of appended blobs. Each blob is
referred to by its offset in the entry array and may be mandatory or optional
(as defined by this specification), where missing entries are denoted by a zero
size. It is therefore not allowed to trim missing optional entries from the end
of the array. The header uses the endianness of the virtual machine.

The header format itself is agnostic of the internal format of the individual
blos it refers to.

##### Version 1.0 {#pvmfw-data-v1-0}

In version 1.0, it describes two blobs:

- entry 0 must point to a valid DICE chain handover (see below)
- entry 1 may point to a [DTBO] to be applied to the pVM device tree. See
  [debug policy][debug_policy] for an example.

##### Version 1.1 {#pvmfw-data-v1-1}

In version 1.1, a third blob is added.

- entry 2 may point to a [DTBO] that describes VM DA DTBO for
  [device assignment][device_assignment].
  pvmfw will provision assigned devices with the VM DTBO.

#### Version 1.2 {#pvmfw-data-v1-2}

In version 1.2, a fourth blob is added.

- entry 3 if present contains the VM reference DT. This defines properties that
  may be included in the device tree passed to a protected VM. pvmfw validates
  that if any of these properties is included in the VM's device tree, the
  property value exactly matches what is in the VM reference DT.

  The bootloader should ensure that the same properties, with the same values,
  are added under the "/avf/reference" node in the host Android device tree.

  This provides a mechanism to allow configuration information to be securely
  passed to the VM via the host. pvmfw does not interpret the content of VM
  reference DT, nor does it apply it to the VM's device tree, it just ensures
  that if matching properties are present in the VM device tree they contain the
  correct values.

  Use-cases of VM reference DT include:

  - Passing the [public key of the Secretkeeper][secretkeeper_key] HAL
    implementation to each VM.

  - Passing the [vendor hashtree digest][vendor_hashtree_digest] to run
    Microdroid with verified vendor image.

[header]: src/config.rs
[DTBO]: https://android.googlesource.com/platform/external/dtc/+/refs/heads/main/Documentation/dt-object-internal.txt
[debug_policy]: ../docs/debug/README.md#debug-policy
[device_assignment]: ../docs/device_assignment.md
[secretkeeper_key]: https://android.googlesource.com/platform/system/secretkeeper/+/refs/heads/main/README.md#secretkeeper-public-key
[vendor_hashtree_digest]: ../microdroid/README.md#verification-of-vendor-image

#### Virtual Platform DICE Chain Handover

The format of the DICE chain entry mentioned above, compatible with the
[`AndroidDiceHandover`][AndroidDiceHandover] defined by the Open Profile for
DICE reference implementation, is described by the following [CDDL][CDDL]:
```
PvmfwDiceHandover = {
  1 : bstr .size 32,     ; CDI_Attest
  2 : bstr .size 32,     ; CDI_Seal
  3 : DiceCertChain,     ; Android DICE chain
}
```

It contains the _Compound Device Identifiers_ (CDIs), used for deriving the
next-stage secret, and a certificate chain, necessary for building the full
[pVM DICE chain][pvm-dice-chain] required by features like
[pVM remote attestation][vm-attestation].

Note that it differs from the `AndroidDiceHandover` defined by the specification
in that its `DiceCertChain` field is mandatory (while optional in the original).

Devices that fully implement DICE should provide a certificate rooted at the
Unique Device Secret (UDS) in a boot stage preceding the pvmfw loader (typically
ABL), in such a way that it would receive a valid `AndroidDiceHandover`, that
can be passed to [`DiceAndroidHandoverMainFlow`][DiceAndroidHandoverMainFlow] along with
the inputs described below.

The recommended DICE inputs at this stage are:

- **Code**: hash of the pvmfw image, hypervisor (`boot.img`), and other target
  code relevant to the secure execution of pvmfw (_e.g._ `vendor_boot.img`)
- **Configuration Data**: any extra input relevant to pvmfw security
- **Authority Data**: must cover all the public keys used to sign and verify the
  code contributing to the **Code** input
- **Mode Decision**: Set according to the [specification][dice-mode]. In
  particular, should only be `Normal` if secure boot is being properly enforced
  (_e.g._ locked device in [Android Verified Boot][AVB])
- **Hidden Inputs**: Factory Reset Secret (FRS, stored in a tamper evident
  storage and changes during every factory reset) or similar that changes as
  part of the device lifecycle (_e.g._ reset)

The resulting `AndroidDiceHandover` is then used by pvmfw in a similar way to
derive another [DICE layer][Layering], passed to the guest through a
`/reserved-memory` device tree node marked as
[`compatible=”google,open-dice”`][dice-dt].

[AVB]: https://source.android.com/docs/security/features/verifiedboot/boot-flow
[AndroidDiceHandover]: https://pigweed.googlesource.com/open-dice/+/42ae7760023/src/android.c#212
[DiceAndroidHandoverMainFlow]: https://pigweed.googlesource.com/open-dice/+/42ae7760023/src/android.c#221
[CDDL]: https://datatracker.ietf.org/doc/rfc8610
[dice-mode]: https://pigweed.googlesource.com/open-dice/+/refs/heads/main/docs/specification.md#Mode-Value-Details
[dice-dt]: https://www.kernel.org/doc/Documentation/devicetree/bindings/reserved-memory/google%2Copen-dice.yaml
[Layering]: https://pigweed.googlesource.com/open-dice/+/refs/heads/main/docs/specification.md#layering-details
[pvm-dice-chain]: ../../docs/pvm_dice_chain.md
[vm-attestation]: ../../docs/vm_remote_attestation.md

### Platform Requirements

pvmfw is intended to run in a virtualized environment according to the `crosvm`
[memory layout][crosvm-mem] for protected VMs and so it expects to have been
loaded at address `0x7fc0_0000` and uses the 2MiB region at address
`0x7fe0_0000` as scratch memory. It makes use of the virtual PCI bus to obtain a
virtio interface to the host and prints its logs through the 16550 UART (address
`0x3f8`).

At boot, pvmfw discovers the running hypervisor in order to select the
appropriate hypervisor calls to share/unshare memory, mark IPA regions as MMIO,
obtain trusted true entropy, and reboot the virtual machine. In particular, it
makes use of the following hypervisor calls:

- Arm [SMC Calling Convention][smccc] v1.1 or above:

    - `SMCCC_VERSION`
    - Vendor Specific Hypervisor Service Call UID Query

- Arm [Power State Coordination Interface][psci] v1.0 or above:

    - `PSCI_VERSION`
    - `PSCI_FEATURES`
    - `PSCI_SYSTEM_RESET`
    - `PSCI_SYSTEM_SHUTDOWN`

- Arm [True Random Number Generator Firmware Interface][smccc-trng] v1.0:

    - `TRNG_VERSION`
    - `TRNG_FEATURES`
    - `TRNG_RND`

- When running under KVM, the pKVM-specific hypervisor interface must provide:

    - `MEMINFO` (function ID `0xc6000002`)
    - `MEM_SHARE` (function ID `0xc6000003`)
    - `MEM_UNSHARE` (function ID `0xc6000004`)
    - `MMIO_GUARD_INFO` (function ID `0xc6000005`)
    - `MMIO_GUARD_ENROLL` (function ID `0xc6000006`)
    - `MMIO_GUARD_MAP` (function ID `0xc6000007`)
    - `MMIO_GUARD_UNMAP` (function ID `0xc6000008`)

[crosvm-mem]: https://crosvm.dev/book/appendix/memory_layout.html
[psci]: https://developer.arm.com/documentation/den0022
[smccc]: https://developer.arm.com/documentation/den0028
[smccc-trng]: https://developer.arm.com/documentation/den0098

## Booting Protected Virtual Machines

### Boot Protocol

As the hypervisor makes pvmfw the entry point of the VM, the initial value of
the registers it receives is configured by the VMM and is expected to follow the
[Linux ABI] _i.e._

- x0 = physical address of device tree blob (dtb) in system RAM.
- x1 = 0 (reserved for future use)
- x2 = 0 (reserved for future use)
- x3 = 0 (reserved for future use)

Images to be verified, which have been loaded to guest memory by the VMM prior
to booting the VM, are described to pvmfw using the device tree (x0):

- the kernel in the `/config` DT node _e.g._

    ```
    / {
        config {
            kernel-address = <0x80200000>;
            kernel-size = <0x1000000>;
        };
    };
    ````

- the (optional) ramdisk in the standard `/chosen` node _e.g._

    ```
    / {
        chosen {
            linux,initrd-start = <0x82000000>;
            linux,initrd-end = <0x82800000>;
        };
    };
    ```

[Linux ABI]: https://www.kernel.org/doc/Documentation/arm64/booting.txt

### Handover ABI

After verifying the guest kernel, pvmfw boots it using the Linux ABI described
above. It uses the device tree to pass [AVF-specific properties][dt.md] and the
DICE chain:

```
/ {
    reserved-memory {
        #address-cells = <0x02>;
        #size-cells = <0x02>;
        ranges;
        dice {
            compatible = "google,open-dice";
            no-map;
            reg = <0x0 0x7fe0000>, <0x0 0x1000>;
        };
    };
};
```

[dt.md]: ../docs/device_trees.md#avf_specific-properties-and-nodes

### Guest Image Signing

pvmfw verifies the guest kernel image (loaded by the VMM) by re-using tools and
formats introduced by the Android Verified Boot. In particular, it expects the
kernel region (see `/config/kernel-{address,size}` described above) to contain
an appended VBMeta structure, which can be generated as follows:

```
avbtool add_hash_footer --image <kernel.bin> \
    --partition_name boot \
    --dynamic_partition_size \
    --key $KEY
```

In cases where a ramdisk is required by the guest, pvmfw must also verify it. To
do so, it must be covered by a hash descriptor in the VBMeta of the kernel:

```
cp <initrd.bin> /tmp/
avbtool add_hash_footer --image /tmp/<initrd.bin> \
    --partition_name $INITRD_NAME \
    --dynamic_partition_size \
    --key $KEY
avbtool add_hash_footer --image <kernel.bin> \
    --partition_name boot \
    --dynamic_partition_size \
    --include_descriptor_from_image /tmp/<initrd.bin> \
    --key $KEY
```

Note that the `/tmp/<initrd.bin>` file is only created to temporarily hold the
hash descriptor to be added to the kernel footer and that the unsigned
`<initrd.bin>` should be passed to the VMM when booting a pVM.

The name of the AVB "partition" for the ramdisk (`$INITRD_NAME`) can be used by
the signer to specify if pvmfw must consider the guest to be debuggable
(`initrd_debug`) or not (`initrd_normal`), which will be reflected in the
certificate of the guest and will affect the secrets being provisioned.

If pVM guest kernels are built and/or packaged using the Android Build system,
the signing described above is recommended to be done through an
`avb_add_hash_footer` Soong module (see [how we sign the Microdroid
kernel][soong-udroid]).

[soong-udroid]: https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/Virtualization/microdroid/Android.bp;l=425;drc=b94a5cf516307c4279f6c16a63803527a8affc6d

## Development

For faster iteration, you can build pvmfw, adb-push it to the device, and use
it directly for a new pVM, without having to flash it to the physical
partition. To do that, the binary image composition performed by ABL described
above must be replicated to produce a single file containing the pvmfw binary
and its configuration data.

As a quick prototyping solution, a valid DICE chain (such as this [test
file][bcc.dat]) can be appended to the `pvmfw.bin` image with `pvmfw-tool`.

```shell
m pvmfw-tool pvmfw_bin
PVMFW_BIN=${ANDROID_PRODUCT_OUT}/system/etc/pvmfw.bin
DICE=${ANDROID_BUILD_TOP}/packages/modules/Virtualization/tests/pvmfw/assets/bcc.dat

pvmfw-tool custom_pvmfw ${PVMFW_BIN} ${DICE}
```

The result can then be pushed to the device. Pointing the system property
`hypervisor.pvmfw.path` to it will cause AVF to use that image as pvmfw:

```shell
adb push custom_pvmfw /data/local/tmp/pvmfw
adb root
adb shell setprop hypervisor.pvmfw.path /data/local/tmp/pvmfw
```

Then run a protected VM, for example:

```shell
adb shell /apex/com.android.virt/bin/vm run-microdroid --protected
```

Note: `adb root` is required to set the system property.

[bcc.dat]: https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/Virtualization/tests/pvmfw/assets/bcc.dat
