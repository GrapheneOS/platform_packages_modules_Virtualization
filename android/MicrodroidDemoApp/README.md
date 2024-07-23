# Microdroid demo app

## Building

```
UNBUNDLED_BUILD_SDKS_FROM_SOURCE=true TARGET_BUILD_APPS=MicrodroidDemoApp m apps_only dist
```

## Installing

You can install the app like this:
```
adb install -t -g out/dist/MicrodroidDemoApp.apk
```

(-t allows it to be installed even though it is marked as a test app, -g grants
the necessary permission.)

You can also explicitly grant or revoke the permission, e.g.
```
adb shell pm grant com.android.microdroid.demo android.permission.MANAGE_VIRTUAL_MACHINE
```

## Running

Run the app by touching the icon on the launcher. Press the `run` button to
start a VM. You can see console output from the VM on the screen. You can stop
the VM by pressing the `stop` button.
