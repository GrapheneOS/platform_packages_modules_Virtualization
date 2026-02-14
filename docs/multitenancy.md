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
      "expected_authority": "3ccdcd8908b0...",
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
      "expected_authority": "7bcf8d9d9de2..."
    }
  ],
  ...
}
```
Note that for expected_authority, use the hex encoding of the sha512 hash of the certificate (for apk) & signing key(for apex).

TODO(b/483292362): Add section for configuring inter-tenant communication & SELinux domain for the tenants
