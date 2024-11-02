# pVM DICE Chain

A VM [DICE][open-dice] chain is a cryptographically linked
[certificates chain][cert-chain] that captures measurements of the VM's
entire execution environment.

This chain should be rooted in the device's ROM and encompass all components
involved in the VM's loading and boot process. To achieve this, we typically
extract measurements of all the components after verified boot at each stage
of the boot process. These measurements are then used to derive a new DICE
certificate describing the next boot stage.

![][pvm-dice-chain-built-img]

[pvm-dice-chain-built-img]: img/pvm-dice-built-during-boot.png
[cert-chain]: https://en.wikipedia.org/wiki/Chain_of_trust

## Vendor responsibility

Vendors are responsible for constructing the first portion of the DICE chain,
from ROM to the pvmfw loader (e.g., ABL). This portion describes the VM's
loading environment. The final certificate in the vendor's chain must include
measurements of pvmfw, the hypervisor, and any other code relevant to pvmfw's
secure execution.

## pVM DICE handover

Vendors then pass this DICE chain, along with its corresponding
[CDI values][dice-cdi], in a handover to pvmfw. The pVM takes over this
handover and extends it with additional nodes describing its own execution
environment.

[dice-cdi]: https://android.googlesource.com/platform/external/open-dice/+/main/docs/specification.md#cdi-values
![][pvm-dice-handover-img]

### Key derivation

Key derivation is a critical step in the DICE handover process within
[pvmfw][pvmfw]. Vendors need to ensure that both pvmfw and their final DICE
node use the same method to derive a key pair from `CDI_Attest` in order to
maintain a valid certificate chain. Pvmfw uses [open-dice][open-dice] with the
following formula:

```
CDI_Attest_pub, CDI_Attest_priv = KDF_ASYM(KDF(CDI_Attest))
```

Where KDF = HKDF-SHA-512 (RFC 5869).

Currently, KDF_ASYM = Ed25519, but EC p-384 and p-256 (RFC 6979) support is
coming soon.

Vendors must use a supported algorithm for the last DICE node to ensure
compatibility and chain integrity.

[pvmfw]: ../guest/pvmfw
[pvm-dice-handover-img]: img/pvm-dice-handover.png
[open-dice]: https://android.googlesource.com/platform/external/open-dice/+/main/docs/specification.md

## Validation

While pvmfw and the Microdroid OS extend the VM DICE chain, they don't
perform comprehensive validation of the chain's structure or its ROM-rooted
origin. The [VM Remote Attestation][vm-attestation] feature is specifically
designed to ensure the validity and ROM-rooted nature of a VM DICE chain.

[vm-attestation]: vm_remote_attestation.md

## Testing

To verify that the DICE handover is successful in pvmfw and eventually the pVM
has a valid DICE chain, you can run the VSR test
`MicrodroidTests#protectedVmHasValidDiceChain`. The test retrieves the DICE
chain from within a Microdroid VM in protected mode and checks the following
properties using the [hwtrust][hwtrust] library:

1. All the fields in the DICE chain conform to
   [Android Profile for DICE][android-open-dice].
2. The DICE chain is a valid certificate chain, where the subject public key in
   each certificate can be used to verify the signature of the next certificate.

[hwtrust]: https://cs.android.com/android/platform/superproject/main/+/main:tools/security/remote_provisioning/hwtrust/
[android-open-dice]: https://android.googlesource.com/platform/external/open-dice/+/refs/heads/main/docs/android.md
