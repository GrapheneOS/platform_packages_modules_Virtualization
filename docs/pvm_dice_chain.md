# pVM DICE Chain

Unlike KeyMint, which only needs a vendor DICE chain, the pVM DICE
chain combines the vendor's DICE chain with additional pVM DICE nodes
describing the protected VM's environment.

![][pvm-dice-chain-img]

The full [RKP VM DICE chain][rkpvm-dice-chain], starting from `UDS_Pub`
rooted in ROM, is sent to the RKP server during
[pVM remote attestation][vm-attestation].

[vm-attestation]: vm_remote_attestation.md
[pvm-dice-chain-img]: img/pvm-dice.png
[rkpvm-dice-chain]: vm_remote_attestation.md#rkp-vm-marker

## Key derivation

Key derivation is a critical step in the DICE handover process within
[pvmfw][pvmfw]. Vendors need to ensure that both pvmfw and their final DICE
node use the same method to derive a key pair from `CDI_Attest` in order to
maintain a valid certificate chain. Pvmfw use [open-dice][open-dice] with the
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
[open-dice]: https://cs.android.com/android/platform/superproject/main/+/main:external/open-dice/

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
