# Android Virtualization Framework (AVF)

Android Virtualization Framework (AVF) provides secure and private execution environments for
executing code. AVF is ideal for security-oriented use cases that require stronger isolation
assurances over those offered by Android’s app sandbox.

Visit [our public doc site](https://source.android.com/docs/core/virtualization) to learn more about
what AVF is, what it is for, and how it is structured. This repository contains source code for
userspace components of AVF.

If you want a quick start, see the [getting started guideline](docs/getting_started.md)
and follow the steps there.

For in-depth explanations about individual topics and components, visit the following links.

AVF components:
* [pVM firmware](pvmfw/README.md)
* [Android Boot Loader (ABL)](docs/abl.md)
* [Microdroid](microdroid/README.md)
* [Microdroid kernel](microdroid/kernel/README.md)
* [Microdroid payload](microdroid/payload/README.md)
* [vmbase](vmbase/README.md)
* [Encrypted Storage](encryptedstore/README.md)

AVF APIs:
* [Java API](java/framework/README.md)
* [VM Payload API](vm_payload/README.md)

How-Tos:
* [Building and running a demo app in Java](demo/README.md)
* [Building and running a demo app in C++](demo_native/README.md)
* [Debugging](docs/debug)
* [Using custom VM](docs/custom_vm.md)
* [Device assignment](docs/device_assignment.md)
* [Huge Pages](docs/hugepages.md)
