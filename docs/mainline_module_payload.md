# Delivery Microdroid pVM payload via Mainline modules

Note: this feature is under development, use it with cauition!

There are several additional challenges when a Microdroid pVM payload is
delivered inside a Mainline module.

## Mainline rollbacks

Mainline modules are expected to be rolled back on a device in case a problem
with a Mainline release has been detected. This doesn't work well with the
rollback protection of Microdroid pVMs - if a payload is updated, then a
previous version of the payload is not allowed to access it's secrets.

To work around this challenge, payloads delivered via Mainline modules are
expected to request
`android.permission.USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION` privileged
permission. Additionally they need to specify a
`android.system.virtualmachine.ROLLBACK_INDEX` property in their manifest, e.g.:

```xml
<uses-permission android:name="android.permission.USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION" />
<application>
  <property android:name="android.system.virtualmachine.ROLLBACK_INDEX" android:value="1" />
</application>
```

If apk manifest has both permission and the property specified then the value of
the `android.system.virtualmachine.ROLLBACK_INDEX` property is used by
`microdroid_manager` when constructing the payload node of the dice chain.

Please check the tests prefixed with `relaxedRollbackProtectionScheme` to get
more context on the behaviour.

