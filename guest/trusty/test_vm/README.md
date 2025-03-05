## test_vm

The Trusty test_vm ought to include the test TAs for the Trusted HALs,
defined under hardware/interfaces/security/see:

- AuthMgr
- Secure Storage
- HWCrypto
- HDCP

The Trusty test_vm also includes the VINTF test which allows to check the vendor
support of the Trusted HALs (version and API hash), against the expected
compatibility matrix for a given Android Dessert Release.
