#!/bin/bash

set -ex

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

show_help() {
  echo "Usage: $0 [OPTION]..."
  echo "Builds images.tar.gz with Debian payload."
  echo "Options:"
  echo "-a ARCH        Architecture of the image [default is host arch: $(uname -m)]"
  echo "-b             Set build id [default is eng-\$(hostname)-\$(date --utc)]"
  echo "-g             Use Debian generic kernel [default is our custom kernel]"
  echo "-h             Print usage and this help message and exit."
  echo "-i IMAGE_NAME  Specify the image name [default is ubuntu:22.04]"
  echo "-s             Leave a shell open if able [default: only if the build fails]"
  echo "-t VIRT_TOP    Specify the virtualization repo top [default is deduced from script location]"
  echo "-u             Set VM boot mode to u-boot [default is to load kernel directly]"
  echo "-w             Save temp work directory in the container [for debugging]"
  echo "-W WORK_DIR    Specify work dir instead of temporarily creating one. Imply -w [for debugging]"
}

ensure_binfmt_misc() {
  if [[ "$arch" != "$(uname -m)" ]]; then
    binfmt_misc="/proc/sys/fs/binfmt_misc/qemu-${arch}"
    if [[ ! -f "${binfmt_misc}" ]]; then
      # Enable multi-arch container by QEMU.
      docker run --rm --privileged multiarch/qemu-user-static --reset -p yes
    fi
  fi
}

parse_options() {
  while getopts "a:b:ghi:st:uwW:" option; do
    case ${option} in
      a)
        arch="$OPTARG"
        ;;
      b)
        build_id="$OPTARG"
        ;;
      g)
        kernel_flag="-g"
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
      u)
        uboot_flag="-u"
        ;;
      w)
        save_workdir_flag="-w"
        ;;
      W)
        mount_work_dir="-v ${OPTARG}:${OPTARG}"
        work_dir_flag="-W ${OPTARG}"
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
}

arch="$(uname -m)"
build_id=$(echo eng-$(hostname)-$(date --utc))
image_name="ubuntu:22.04"
kernel_flag=
save_workdir_flag=
shell="|| bash"
uboot_flag=
virt_repo_top="${SCRIPT_DIR}/../../"
mount_work_dir=
work_dir_flag=

parse_options "$@"
ensure_binfmt_misc

docker run --privileged $interactive \
  $mount_work_dir \
  -v /dev:/dev \
  -v "$virt_repo_top:/root/Virtualization" \
  -v /var/log/fai:/var/log/fai \
  --workdir /root/Virtualization/build/debian \
  "$image_name" \
  bash -c "./build.sh -a $arch $kernel_flag $uboot_flag $save_workdir_flag $work_dir_flag -b \"$build_id\" $shell"
