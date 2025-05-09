#!/bin/bash

set -ex

show_help() {
  echo "Usage: sudo $0 [OPTION]..."
  echo "Builds a debian image and save it to image.raw."
  echo "Options:"
  echo "-b BUILD_TOP   Specify build top [default is generated from \$ANDROID_BUILD_TOP]"
  echo "-i IMAGE_NAME  Specify the image name [default is ubuntu:22.04]"
  echo "-h             Print usage and this help message and exit."
  echo "-a ARCH        Architecture of the image [default is host arch: $(uname -m)]"
  echo "-g             Use Debian generic kernel [default is our custom kernel]"
  echo "-r             Release mode build"
  echo "-s             Leave a shell open if able [default: only if the build fails]"
  echo "-u             Set VM boot mode to u-boot [default is to load kernel directly]"
  echo "-w             Save temp work directory in the container [for debugging]"
}

arch="$(uname -m)"
kernel_flag=
release_flag=
save_workdir_flag=
shell="|| bash"
uboot_flag=
build_top=
image_name="ubuntu:22.04"

while getopts "a:b:i:ghrsuw" option; do
  case ${option} in
    a)
      arch="$OPTARG"
      ;;
    b)
      build_top="$OPTARG"
      ;;
    i)
      image_name="$OPTARG"
      ;;
    g)
      kernel_flag="-g"
      ;;
    h)
      show_help ; exit
      ;;
    r)
      release_flag="-r"
      ;;
    s)
      shell="; bash"
      ;;
    u)
      uboot_flag="-u"
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

if [ -z "$build_top" ]; then
  if [ -z "$ANDROID_BUILD_TOP" ] ; then
    echo "Cannot find build top"
    echo "Please use -b option or \'lunch\' an Android target for autodetect"
    exit 1
  fi
  build_top="$ANDROID_BUILD_TOP/packages/modules/Virtualization"
fi

if [[ -t 0 ]]; then
  interactive="-it"
else
  echo "Not an interactive shell. Can't leave a shell open."
  shell=""
fi

docker run --privileged $interactive \
  -v /dev:/dev \
  -v "$build_top:/root/Virtualization" \
  -v /var/log/fai:/var/log/fai \
  --workdir /root/Virtualization/build/debian \
  "$image_name" \
  bash -c "./build.sh -a $arch $release_flag $kernel_flag $uboot_flag $save_workdir_flag $shell"
