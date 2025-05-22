#!/bin/bash

set -ex

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

show_help() {
  echo "Usage: $0 [OPTION]..."
  echo "Builds Debian packages for our custom kernel."
  echo "Options:"
  echo "-a ARCH        Architecture of the image [default is host arch: $(uname -m)]"
  echo "-h             Print usage and this help message and exit."
  echo "-i IMAGE_NAME  Specify the image name [default is ubuntu:22.04]"
  echo "-s             Leave a shell open if able [default: only if the build fails]"
  echo "-t VIRT_TOP    Specify the virtualization repo top [default is deduced from script location]"
  echo "-w             Save temp work directory in the container [for debugging]"
}

arch="$(uname -m)"
image_name="ubuntu:22.04"
save_workdir_flag=
shell="|| bash"
virt_repo_top="${SCRIPT_DIR}/../../"

while getopts "a:hi:st:w" option; do
  case ${option} in
    a)
      arch="$OPTARG"
      ;;
    h)
      show_help ; exit
      ;;
    i)
      image_name="$OPTARG"
      ;;
    s)
      shell="; bash"
      ;;
    t)
      virt_repo_top="$OPTARG"
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

if [[ -t 0 ]]; then
  interactive="-it"
else
  echo "Not an interactive shell. Can't leave a shell open."
  shell=""
fi

docker run $interactive \
  -v /dev:/dev \
  -v "$virt_repo_top:/root/Virtualization" \
  --workdir /root/Virtualization/build/debian \
  "$image_name" \
  bash -c "./build_custom_kernel.sh -a $arch $save_workdir_flag $shell"
