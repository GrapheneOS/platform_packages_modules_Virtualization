#!/system/bin/sh

function round_up() {
  num=$1
  div=$2
  echo $((( (( ${num} / ${div} ) + 1) * ${div} )))
}

function install() {
  user=$(cmd user get-main-user)
  src_dir=/data/media/${user}/ferrochrome/
  dst_dir=/data/local/tmp/

  cat $(find ${src_dir} -name "images.tar.gz*" | sort) | tar xz -C ${dst_dir}
  cp -u ${src_dir}vm_config.json ${dst_dir}
  chmod 666 ${dst_dir}*

  # increase the size of state.img to the multiple of 4096
  num_blocks=$(du -b -K ${dst_dir}state.img | cut -f 1)
  required_num_blocks=$(round_up ${num_blocks} 4)
  additional_blocks=$((( ${required_num_blocks} - ${num_blocks} )))
  dd if=/dev/zero bs=512 count=${additional_blocks} >> ${dst_dir}state.img

  rm ${src_dir}images.tar.gz*
  rm ${src_dir}vm_config.json
}

setprop debug.custom_vm_setup.done false
install
setprop debug.custom_vm_setup.start false
setprop debug.custom_vm_setup.done true
