#!/system/bin/sh

function copy_files() {
  cp -u /sdcard/vm_config.json /data/local/tmp
  cp -u /sdcard/vmlinuz /data/local/tmp
  cp -u /sdcard/chromiumos_test_image.bin /data/local/tmp
  chmod 666 /data/local/tmp/vm_config.json
  chmod 666 /data/local/tmp/chromiumos_test_image.bin
  chmod 666 /data/local/tmp/vmlinuz
}
setprop debug.custom_vm_setup.done false
copy_files
pm grant com.google.android.virtualization.vmlauncher android.permission.USE_CUSTOM_VIRTUAL_MACHINE
setprop debug.custom_vm_setup.start false
setprop debug.custom_vm_setup.done true