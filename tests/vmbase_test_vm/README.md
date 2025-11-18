A barebone libvmbase-based VM image that can be used to test functionality not
available to Java @SystemApis / microdroid-based VMs, e.g.:

1. LL-NDK APIs
2. Low level functionality (e.g. SMCs, guest HVCs, etc.) that is hard to test
   via microdroid-based VMs.

This VM supports a very basic IPC mechanism overs vsock. For the
definition of the supported messages see messages/src/lib.rs.
