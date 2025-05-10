#!/bin/bash

set -ex

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

show_help() {
  echo "Usage: sudo $0 [OPTION]..."
  echo "Builds a debian image and save it to image.raw."
  echo "Options:"
  echo "-t VIRT_TOP    Specify the virtualization repo top [default is deduced from script location]"
  echo "-i IMAGE_NAME  Specify the image name [default is ubuntu:22.04]"
  echo "-h             Print usage and this help message and exit."
  echo "-a ARCH        Architecture of the image [default is host arch: $(uname -m)]"
  echo "-g             Use Debian generic kernel [default is our custom kernel]"
  echo "-s             Leave a shell open if able [default: only if the build fails]"
  echo "-u             Set VM boot mode to u-boot [default is to load kernel directly]"
  echo "-w             Save temp work directory in the container [for debugging]"
  echo "-b             Set build id [default is eng-\$(hostname)-\$(date --utc)]"
}

arch="$(uname -m)"
virt_repo_top="${SCRIPT_DIR}/../../"
kernel_flag=
save_workdir_flag=
shell="|| bash"
uboot_flag=
image_name="ubuntu:22.04"
build_id=$(echo eng-$(hostname)-$(date --utc))

while getopts "a:b:i:ghsuwt:" option; do
  case ${option} in
    a)
      arch="$OPTARG"
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
    s)
      shell="; bash"
      ;;
    u)
      uboot_flag="-u"
      ;;
    w)
      save_workdir_flag="-w"
      ;;
    b)
      build_id="$OPTARG"
      ;;
    t)
      virt_repo_top="$OPTARG"
      ;;
    *)
      echo "Invalid option: $OPTARG" ; exit 1
      ;;
  esac
done

if [[ "$arch" != "aarch64" && "$arch" != "x86_64" ]]; then
  echo "Invalid architecture: $arch" ; exit 1
fi

if [[ -t 0 ]]; then
  interactive="-it"
else
  echo "Not an interactive shell. Can't leave a shell open."
  shell=""
fi

docker run --privileged $interactive \
  -v /dev:/dev \
  -v "$virt_repo_top:/root/Virtualization" \
  -v /var/log/fai:/var/log/fai \
  --workdir /root/Virtualization/build/debian \
  "$image_name" \
  bash -c "./build.sh -a $arch $kernel_flag $uboot_flag $save_workdir_flag -b $build_id $shell"
