#!/system/bin/sh

function round_up() {
  num=$1
  div=$2
  echo $((( (( ${num} / ${div} ) + 1) * ${div} )))
}

function install() {
  src_dir=$(getprop debug.custom_vm_setup.path)
  src_dir=${src_dir/#\/storage\/emulated\//\/data\/media\/}
  dst_dir=/data/local/tmp/

  cat $(find ${src_dir} -name "images.tar.gz*" | sort) | tar xz -C ${dst_dir}
  cp -u ${src_dir}/vm_config.json ${dst_dir}
  chmod 666 ${dst_dir}/*

  if [ -f ${dst_dir}state.img ]; then
    # increase the size of state.img to the multiple of 4096
    num_blocks=$(du -b -K ${dst_dir}state.img | cut -f 1)
    required_num_blocks=$(round_up ${num_blocks} 4)
    additional_blocks=$((( ${required_num_blocks} - ${num_blocks} )))
    dd if=/dev/zero bs=512 count=${additional_blocks} >> ${dst_dir}state.img
  fi
  rm ${src_dir}/images.tar.gz*
  rm ${src_dir}/vm_config.json
}

setprop debug.custom_vm_setup.done false
install
setprop debug.custom_vm_setup.start false
setprop debug.custom_vm_setup.done true
