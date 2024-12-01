#!/bin/bash

if [ -z "$ANDROID_BUILD_TOP" ]; then echo "forgot to source build/envsetup.sh?" && exit 1; fi

arch=aarch64
release_flag=
save_workdir_flag=

while getopts "a:rw" option; do
  case ${option} in
    a)
      if [[ "$OPTARG" != "aarch64" && "$OPTARG" != "x86_64" ]]; then
        echo "Invalid architecture: $OPTARG"
        exit
      fi
      arch="$OPTARG"
      ;;
    r)
      release_flag="-r"
      ;;
    w)
      save_workdir_flag="-w"
      ;;
    *)
      echo "Invalid option: $OPTARG"
      exit
      ;;
  esac
done

docker run --privileged -it -v /dev:/dev \
  -v "$ANDROID_BUILD_TOP/packages/modules/Virtualization:/root/Virtualization" \
  --workdir /root/Virtualization/build/debian \
  ubuntu:22.04 \
  bash -c "/root/Virtualization/build/debian/build.sh -a $arch $release_flag $save_workdir_flag || bash"
