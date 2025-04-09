#!/bin/bash

arch="$(uname -m)"
save_workdir_flag=

while getopts "a:w" option; do
  case ${option} in
    a)
      arch="$OPTARG"
      ;;
    w)
      save_workdir_flag="-w"
      ;;
    *)
      echo "Invalid option: $OPTARG" ; exit 1
      ;;
  esac
done

if [[ "$arch" != "aarch64" && "$arch" != "x86_64" ]]; then
  echo "Invalid architecture: $arch" ; exit 1
fi

if [ -z "$ANDROID_BUILD_TOP" ] ; then
  echo '`ANDROID_BUILD_TOP` is undefined.'
  echo 'Please `lunch` an Android target, or manually set the variable.'
  exit 1
fi

docker run -it -v /dev:/dev \
  -v "$ANDROID_BUILD_TOP/packages/modules/Virtualization:/root/Virtualization" \
  --workdir /root/Virtualization/build/debian \
  ubuntu:22.04 \
  bash -c "./build_custom_kernel.sh -a $arch $save_workdir_flag || bash"
