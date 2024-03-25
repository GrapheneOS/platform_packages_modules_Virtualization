# VM launcher app

## Building & Installing

Add `VmLauncherApp` into `PRODUCT_PACKAGES` and then `m`

You can also explicitly grant or revoke the permission, e.g.
```
adb shell pm grant com.android.virtualization.vmlauncher android.permission.USE_CUSTOM_VIRTUAL_MACHINE
adb shell pm grant com.android.virtualization.vmlauncher android.permission.MANAGE_VIRTUAL_MACHINE
```

## Running

Copy vm config json file to /data/local/tmp/vm_config.json.
And then, run the app, check log meesage.
