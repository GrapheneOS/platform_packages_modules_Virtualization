The stub.c file here is a checked-in version of a generated code file.

It is not needed when building a payload client in the Android build
system. The build system will automatically generated it (from
libvm_payload.map.txt) and then compile it to form the stub version of
libvm_payload.so. Clients link against the stub, but at runtime they
will use the real libvm_payload.so provided by Microdroid.

This file is here to support non-Android build systems, to allow a
suitable stub libvm_payload.so to be built.

To update this file, something like the following should work:

lunch aosp_arm64-eng
m MicrodroidTestNativeLib

The generated stub file can then be found at out/soong/.intermediates/packages/modules/Virtualization/libs/libvm_payload/libvm_payload/android_arm64_armv8-a_shared_current/gen/stub.c
