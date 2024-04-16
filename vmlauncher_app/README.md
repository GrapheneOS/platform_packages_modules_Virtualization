# VM launcher app

## Building

This app is now part of the virt APEX.

## Enabling

This app is disabled by default. To re-enable it, execute the following command.

```
adb root
adb shell pm enable com.android.virtualization.vmlauncher/.MainActivity
```

## Running

Copy vm config json file to /data/local/tmp/vm_config.json.
And then, run the app, check log meesage.
