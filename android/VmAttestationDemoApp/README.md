# VmAttestationDemoApp

## Overview

The *VmAttestationDemoApp* is an Android application that provides a practical
demonstration of how to interact with the VM Attestation APIs. This app focuses
on the payload of the Android app and the payload performs two main tasks:
requesting attestation and validating the attestation result.

## Building

To build the VmAttestationDemoApp, use the following command:

```
m VmAttestationDemoApp
```

## Installing

To install the app on your device, execute the following command:

```
adb install $ANDROID_PRODUCT_OUT/system/app/VmAttestationDemoApp/VmAttestationDemoApp.apk
```

## Running

Before running the app, make sure that the device has an internet connection and
that the remote provisioning host is not empty. You can use the following
command to check the remote provisioning host:

```
$ adb shell getprop remote_provisioning.hostname
remoteprovisioning.googleapis.com
```

Once you have confirmed the remote provisioning host, you can run the app using
the following command:

```
TEST_ROOT=/data/local/tmp/virt && adb shell /apex/com.android.virt/bin/vm run-app \
  --config-path assets/config.json --debug full \
  $(adb shell pm path com.android.virt.vm_attestation.demo | cut -c 9-) \
  $TEST_ROOT/VmAttestationDemoApp.apk.idsig \
  $TEST_ROOT/instance.vm_attestation.debug.img \
  --instance-id-file $TEST_ROOT/instance_id \
  --protected
```

Please note that remote attestation is only available for protected VMs.
Therefore, ensure that the VM is launched in protected mode using the
`--protected` flag.

If everything is set up correctly, you should be able to see the attestation
result printed out in the VM logs.
