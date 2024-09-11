# VM Remote Attestation

## Introduction

In today's digital landscape, where security threats are ever-evolving, ensuring
the authenticity and integrity of VMs is paramount. This is particularly crucial
for sensitive applications, such as those running machine learning models, where
guaranteeing a trusted and secure execution environment is essential.

VM remote attestation provides a powerful mechanism for *protected VMs* (pVMs)
to prove their trustworthiness to a third party. This process allows a pVM to
demonstrate that:

-   All its components, including firmware, operating system, and software, are
    valid and have not been tampered with.
-   It is running on a valid device trusted by the
    [Remote Key Provisioning][rkp] (RKP) backend, such as Google.

[rkp]: https://source.android.com/docs/core/ota/modular-system/remote-key-provisioning

## Design

The process of pVM remote attestation involves the use of a lightweight
intermediate VM known as the [RKP VM][rkpvm]. It allows us to divide the
attestation process into two parts:

1.  Attesting the RKP VM against the RKP server.
2.  Attesting the pVM against the RKP VM.

[rkpvm]: https://android.googlesource.com/platform/packages/modules/Virtualization/+/main/docs/service_vm.md

### RKP VM attestation

The RKP VM is recognized and attested by the RKP server, which acts as a trusted
entity responsible for verifying the [DICE chain][open-dice] of the RKP VM. This
verification ensures that the RKP VM is operating on a genuine device.
Additionally, the RKP VM is validated by the pVM Firmware, as part of the
verified boot process.

During the validation process, the RKP server compares the root public key of the
DICE chain with the ones registered in the RKP database. Additionally, the server
examines the presence of the [RKP VM marker][rkpvm-marker] within the DICE
certificates to determine the origin of the chain, confirming that it indeed
originates from the RKP VM. For more detailed information about the RKP VM
DICE chain validation, please refer to the [Remote Provisioning HAL][rkp-hal]
spec.

[open-dice]: https://android.googlesource.com/platform/external/open-dice/+/main/docs/android.md

### pVM attestation

Once the RKP VM is successfully attested, it acts as a trusted platform to
attest pVMs. Leveraging its trusted status, the RKP VM validates the integrity
of each [pVM DICE chain][pvm-dice-chain] by comparing it against its own DICE
chain. This validation process ensures that the pVMs are running in the expected
VM environment and certifies the payload executed within each pVM. Currently,
only Microdroid VMs are supported.

[pvm-dice-chain]: ./pvm_dice_chain.md

## API

To request remote attestation of a pVM, the [VM Payload API][api]
`AVmPayload_requestAttestation(challenge)` can be invoked within the pVM
payload.

For detailed information and usage examples, please refer to the
[demo app][demo].

[api]: https://android.googlesource.com/platform/packages/modules/Virtualization/+/main/libs/libvm_payload/README.md
[demo]: https://android.googlesource.com/platform/packages/modules/Virtualization/+/main/android/VmAttestationDemoApp

## Output

Upon successful completion of the attestation process, a pVM receives an
RKP-backed certificate chain and an attested private key that is exclusively
known to the pVM. This certificate chain includes a leaf certificate covering
the attested public key. Notably, the leaf certificate features a new extension
with the OID `1.3.6.1.4.1.11129.2.1.29.1`, specifically designed to describe the
pVM payload for third-party verification.

The extension format is as follows:

```
AttestationExtension ::= SEQUENCE {
    attestationChallenge       OCTET_STRING,
    isVmSecure                 BOOLEAN,
    vmComponents               SEQUENCE OF VmComponent,
}

VmComponent ::= SEQUENCE {
    name               UTF8String,
    securityVersion    INTEGER,
    codeHash           OCTET STRING,
    authorityHash      OCTET STRING,
}
```

In `AttestationExtension`:

-   The `attestationChallenge` field represents a challenge provided by the
    third party. It is passed to `AVmPayload_requestAttestation()` to ensure
    the freshness of the certificate.
-   The `isVmSecure` field indicates whether the attested pVM is secure. It is
    set to true only when all the DICE certificates in the pVM DICE chain are in
    normal mode.
-   The `vmComponents` field contains a list of all the APKs and apexes loaded
    by the pVM. These components are extracted from the config descriptor of the
    last DiceChainEntry of the pVM DICE chain. Refer to
    [dice_for_avf_guest.cddl][dice_for_avf_guest_cddl] for more information.

[dice_for_avf_guest_cddl]: https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/Virtualization/dice_for_avf_guest.cddl

## To Support It

VM remote attestation is a strongly recommended feature from Android V. To
support it, you only need to provide a valid VM DICE chain satisfying the
following requirements:

- The DICE chain must have a UDS-rooted public key registered at the RKP
  factory.
- The DICE chain must use [RKP VM markers][rkpvm-marker] to help identify the
  RKP VM as required by the [remote provisioning HAL][rkp-hal].

### RKP VM marker

To support VM remote attestation, vendors must include an RKP VM marker in their
DICE certificates. This marker should be present from the early boot stage
within the TEE and continue through to the last DICE certificate before
[pvmfw][pvmfw] takes over.

![RKP VM DICE chain][rkpvm-dice-chain]

Pvmfw will add an RKP VM marker when it's launching an RKP VM. The __continuous
presence__ of this marker throughout the chain allows the RKP server to clearly
identify legitimate RKP VM DICE chains.

This mechanism also serves as a security measure. If an attacker tries to launch
a malicious guest OS or payload, their DICE chain will be rejected by the RKP
server because it will lack the RKP VM marker that pvmfw would have added in a
genuine RKP VM boot process.

[pvmfw]: ../guest/pvmfw/README.md
[rkpvm-dice-chain]: img/rkpvm-dice-chain.png

## To Disable It

The feature is enabled by default. To disable it, you have two options:

1. Set `PRODUCT_AVF_REMOTE_ATTESTATION_DISABLED` to `true` in your Makefile to
   disable the feature at build time.

2. Set the system property `avf.remote_attestation.enabled` to `0` to disable
   the feature at boot time by including the following line in vendor init:
   `setprop avf.remote_attestation.enabled 0`.

If you don't set any of these variables, VM remote attestation will be enabled
by default.

[rkpvm-marker]: https://pigweed.googlesource.com/open-dice/+/HEAD/docs/android.md#configuration-descriptor
[rkp-hal]: https://android.googlesource.com/platform/hardware/interfaces/+/main/security/rkp/README.md#hal
