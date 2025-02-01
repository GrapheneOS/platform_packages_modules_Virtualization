# Delivery Microdroid pVM payload via Mainline modules

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
permission.

TODO(ioffe): add more context on how permission is used once the implementation
is done.
