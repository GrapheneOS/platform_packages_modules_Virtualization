# Encrypted Storage

Since Android U, AVF (with Microdroid) supports Encrypted Storage which is the storage solution
in a VM. Within a VM, this is mounted at a path that can be retrieved via the [`AVmPayload_getEncryptedStoragePath()`][vm_payload_api].
Any data written in encrypted storage is persisted and is available next time the VM is run.

Encrypted Storage is backed by a para-virtualized block device on the guest which is further
backed by a disk image file in the host. The block device is formatted with an ext4 filesystem.

## Security

Encrypted Storage uses block level encryption layer (Device-Mapper's "crypt" target) using a key
derived from the VM secret and AES256 cipher with HCTR2 mode. The Block level encryption ensures
the filesystem is also encrypted.

### Integrity
Encrypted Storage does not offer the level of integrity offered by primitives such as
authenticated encryption/dm-integrity/RPMB. That level of integrity comes with substantial
disk/performance overhead. Instead, it uses HCTR2 which is a super-pseudorandom
permutation encryption mode, this offers better resilience against malleability attacks (than other
modes such as XTS).

## Encrypted Storage and Updatable VMs

With [Updatable VM feature][updatable_vm] shipping in Android V, Encrypted Storage can be accessed
even after OTA/updates of boot images and apks. This requires chipsets to support [Secretkeeper HAL][sk_hal].


[vm_payload_api]: https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/Virtualization/libs/libvm_payload/include/vm_payload.h;l=2?q=vm_payload%2Finclude%2Fvm_payload.h&ss=android%2Fplatform%2Fsuperproject%2Fmain
[updatable_vm]: https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/Virtualization/docs/updatable_vm.md
[sk_hal]: https://cs.android.com/android/platform/superproject/main/+/main:system/secretkeeper/README.md
