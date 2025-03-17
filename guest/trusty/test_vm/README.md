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

### instructions

`atest -s <device-serial-port> VtsSeeHalTargetTest

### test_vm console

The test_vm console can be retrieved from `/data/local/tmp/trusty_test_vm/logs/console.log`.
The script `trusty-vm-laucher.sh` uses `/apex/com.android.virt/bin/vm run` with the option
`--console` to store the console log.

This log can be consulted when the tests are running and will be uploaded
by the Tradefed FilePullerLogCollector runner (see AndroidTest.xml).
