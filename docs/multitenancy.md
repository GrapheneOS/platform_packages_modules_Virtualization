# Multitenancy in AVF

From Android 26Q2, AVF supports multitenancy, which allows multiple tenants applications to run in a single VM. This is useful
in scenarios where you want to isolate different tenants from each other, while still allowing them to share the same VM.

## Configuring a Multitenant VM

We introduce TenancyConfig, which is a signed declaration of trusted cohabitation by the VM owner. This essentially is a description of each of the tenants that will be allowed in the VM, any other payload not described in this should be discarded by pVM instance. This config will be signed by the use case owner & is reflected in the pVM certificates (DICE chains). Concretely, this is the payload config (JSON) file, within tha APK  typically set using `VirtualMachineConfig#setPayloadConfigPath`.


Here is an example of a tenancy config. Use such a tenancy config to configure your VM!

```json
{
  "tenants": [
    {
      "package": "apk",
      "name": "com.android.microdroid.test",
      "min_version": 36,
      "expected_authority": {
        "dev-keys": "3ccdcd8908b0...",
        "test-keys": "ccd8908b0d...",
        "release-keys": "7bcf8d9d9d..."
      },
      "task": {
        "type": "microdroid_launcher",
        "command": "MicrodroidTestNativeLib.so",
        "selinux_type": "appsearch_tenant"
      }
    },
    {
      "package": "apex",
      "name": "com.android.virt",
      "min_version": 1,
      "expected_authority": {
        "dev-keys": "f8d9d9de2...",
        "test-keys": "8d9d9de2f...",
        "release-keys": "1a54c4ac..."
      }
    },
    {
      "package": "apk",
      "name": "com.android.othertest",
      "min_version": 1,
      "expected_authority": {
        "dev-keys": "a_single_key_for_all_builds",
        "test-keys": "a_single_key_for_all_builds",
        "release-keys": "a_single_key_for_all_builds"
      }
    },
    {
      "package": "apex",
      "name": "com.android.anothervirt",
      "min_version": 1,
      "expected_authority": {
        "dev-keys": "another_single_key_for_all_builds",
        "test-keys": "another_single_key_for_all_builds",
        "release-keys": "another_single_key_for_all_builds"
      }
    }
  ],
  ...
}
```
1. `min_version` and `expected_authority` are mandatory fields for each tenant.
2. For `expected_authority`, use the hex encoding of the SHA-512 hash of the certificate (for APK) or the signing key (for APEX).

TODO(b/483292362): Add section for configuring inter-tenant communication & SELinux domain for the tenants
