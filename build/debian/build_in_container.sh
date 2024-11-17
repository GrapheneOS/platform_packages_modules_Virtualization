#!/bin/bash

if [ -z "$ANDROID_BUILD_TOP" ]; then echo "forgot to source build/envsetup.sh?" && exit 1; fi

arch=aarch64
release_flag=
while getopts "ra:" option; do
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
    *)
      echo "Invalid option: $OPTARG"
      exit
      ;;
  esac
done

docker run --privileged -it --workdir /root/Virtualization/build/debian -v \
  "$ANDROID_BUILD_TOP/packages/modules/Virtualization:/root/Virtualization" -v \
  /dev:/dev ubuntu:22.04 /root/Virtualization/build/debian/build.sh -a "$arch" $release_flag
