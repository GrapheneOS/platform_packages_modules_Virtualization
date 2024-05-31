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

[rkpvm]: https://android.googlesource.com/platform/packages/modules/Virtualization/+/main/service_vm/README.md

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
[rkpvm-marker]: https://android.googlesource.com/platform/external/open-dice/+/main/docs/android.md#Configuration-descriptor
[rkp-hal]: https://android.googlesource.com/platform/hardware/interfaces/+/main/security/rkp/README.md

### pVM attestation

Once the RKP VM is successfully attested, it acts as a trusted platform to
attest pVMs. Leveraging its trusted status, the RKP VM validates the integrity
of each pVM's DICE chain by comparing it against its own DICE chain. This
validation process ensures that the pVMs are running in the expected VM
environment and certifies the payload executed within each pVM. Currently, only
Microdroid VMs are supported.

## API

To request remote attestation of a pVM, the [VM Payload API][api]
`AVmPayload_requestAttestation(challenge)` can be invoked within the pVM
payload.

For detailed information and usage examples, please refer to the
[demo app][demo].

[api]: https://android.googlesource.com/platform/packages/modules/Virtualization/+/main/vm_payload/README.md
[demo]: https://android.googlesource.com/platform/packages/modules/Virtualization/+/main/service_vm/demo_apk

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
    by the pVM.

## To Support It

VM remote attestation is a strongly recommended feature from Android V. To support
it, you only need to provide a valid VM DICE chain satisfying the following
requirements:

- The DICE chain must have a UDS-rooted public key registered at the RKP factory.
- The DICE chain should have RKP VM markers that help identify RKP VM as required
  by the [remote provisioning HAL][rkp-hal-markers].

The feature is enabled by default. To disable it, you can set
`PRODUCT_AVF_REMOTE_ATTESTATION_DISABLED` to true in your Makefile.

[rkp-hal-markers]: https://android.googlesource.com/platform/hardware/interfaces/+/main/security/rkp/README.md#hal
