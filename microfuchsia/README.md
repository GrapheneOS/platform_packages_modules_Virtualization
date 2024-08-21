# Microfuchsia

Microfuchsia is an experimental solution for running trusted applications on
pkvm using the Android Virtualization Framework (AVF).

# How to use

Add the `com.android.microfuchsia` apex to your product.

```
PRODUCT_PACKAGES += com.android.microfuchsia
```

Define and add a `com.android.microfuchsia.images` apex to hold the images.

```
PRODUCT_PACKAGES += com.android.microfuchsia.images
```

This apex must have a prebuilt `fuchsia.zbi` in `/etc/fuchsia.zbi` and a boot
shim in `/etc/linux-arm64-boot-shim.bin`.

# Using the console

This command will open the console for the first VM running in AVF, and can be
used to connect to the microfuchsia console.

```
adb shell -t /apex/com.android.virt/bin/vm console
```
