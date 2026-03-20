#!/bin/bash
#
# ==============================================================================
# Ferrochrome Internal Build Script (Container-side)
# ==============================================================================
#
# This script runs inside the Docker builder container to perform the actual
# guest image construction process.
#
# Key Responsibilities:
# 1. Downloads and verifies the Debian cloud image using SHA512 checksums.
# 2. Compiles guest-side utilities (ttyd and Rust-based agents).
# 3. Customizes the guest root filesystem by chrooting into it.
# 4. Integrates the Android Common Kernel and packages the final images.tar.gz.
#
# Usage: sudo ./build_internal.sh [OPTIONS] [OUTPUT_FILE]
# ==============================================================================

set -eo pipefail

### --- Configuration & Defaults --- ###

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
DEBIAN_VERSION="trixie"
FINAL_OUTPUT_FILE="images.tar.gz"

# Default build parameters (usually overridden by build.sh)
TARGET_ARCH="$(uname -m)"
DEBIAN_BUILD_ID="eng-1000000-$(date --utc +'%a %b %d %H:%M:%S %Z %Y')"
KERNEL_BUILD_ID=""
SAVE_WORKDIR=0
MAY_SKIP_BUILD=0

# Path-related global variables (initialized in initialize_paths)
WORKDIR=""
DEBIAN_BASE_DIR=""
CLOUD_INIT_DIR=""
CHROOT_TTYD_DIR=""
RAW_DISK_IMG=""
ROOT_PART_FILE=""

### --- Utilities --- ###

# Log a step title in blue
log_step() { echo -e "\n\033[1;34m[STEP]\033[0m $1"; }

# Log an informational message in green
log_info() { echo -e "\033[1;32m[INFO]\033[0m $1"; }

# Log an error message in red and exit
error() { echo -e "\033[1;31m[ERROR]\033[0m $1" >&2; exit 1; }

# Print help message
show_help() {
  echo "Usage: sudo $0 [OPTION]... [FILE]"
  echo "Builds a debian image and saves it to FILE. [sudo is required]"
  echo ""
  echo "Options:"
  echo "-a ARCH       Architecture of the image [default: $TARGET_ARCH]"
  echo "-b BUILD_ID   Set build id of the debian image"
  echo "-k KERNEL_ID  Build ID for kernel [default: latest from CI]"
  echo "-w            Save temp work directory [for debugging]"
  echo "-W WORK_DIR   Specify work dir instead of temporarily creating one."
}

### --- Functions --- ###

# Parse command line options and set architecture-specific variables
parse_options() {
  while getopts "a:b:k:hwW:" option; do
    case ${option} in
      a) TARGET_ARCH="$OPTARG" ;;
      b) DEBIAN_BUILD_ID="$OPTARG" ;;
      k) KERNEL_BUILD_ID="$OPTARG" ;;
      h) show_help ; exit ;;
      w) SAVE_WORKDIR=1 ;;
      W)
        WORKDIR="${OPTARG%/}"
        SAVE_WORKDIR=1
        MAY_SKIP_BUILD=1
        ;;
      *) error "Invalid option: $OPTARG" ;;
    esac
  done

  case "$TARGET_ARCH" in
    aarch64) DEBIAN_ARCH="arm64"; VMLINUZ_NAME="Image" ;;
    x86_64)  DEBIAN_ARCH="amd64"; VMLINUZ_NAME="bzImage" ;;
    *) error "Invalid architecture: $TARGET_ARCH" ;;
  esac

  # Use optional positional argument as output filename
  if [[ "${*:$OPTIND:1}" ]]; then
    FINAL_OUTPUT_FILE="${*:$OPTIND:1}"
  fi
}

# Initialize all global path variables
initialize_paths() {
  if [[ -z "${WORKDIR}" ]]; then
    WORKDIR=$(mktemp -d)
  else
    mkdir -p "${WORKDIR}"
  fi

  DEBIAN_BASE_DIR="${WORKDIR}/debian_cloud_image"
  CLOUD_INIT_DIR="${DEBIAN_BASE_DIR}/cidata"
  CHROOT_TTYD_DIR="${DEBIAN_BASE_DIR}/chroot_ttyd"
  RAW_DISK_IMG="${DEBIAN_BASE_DIR}/disk.raw"
  ROOT_PART_FILE="${WORKDIR}/root_part"
}

# Download the Debian cloud image and verify its integrity using SHA512
fetch_debian_image() {
  if [[ "$MAY_SKIP_BUILD" == 1 && -f "${RAW_DISK_IMG}" ]]; then
    log_info "Skipping Debian image download: ${RAW_DISK_IMG} already exists."
    return
  fi

  log_step "Downloading and verifying Debian cloud image (${DEBIAN_ARCH})..."

  local remote_img_name="debian-13-genericcloud-${DEBIAN_ARCH}.tar.xz"
  local url_base="https://cloud.debian.org/images/cloud/${DEBIAN_VERSION}/latest"
  local url="${url_base}/${remote_img_name}"
  local sha_url="${url_base}/SHA512SUMS"
  local local_cache_dir="/mnt/image-cache"
  local local_cached_img="${local_cache_dir}/${remote_img_name}"
  local local_cached_sha="${local_cache_dir}/SHA512SUMS"

  mkdir -p "${DEBIAN_BASE_DIR}" "${local_cache_dir}"

  # Get the latest expected checksum
  wget -q -O "${local_cached_sha}" "${sha_url}"
  local expected_sha=$(grep "${remote_img_name}" "${local_cached_sha}" | awk '{print $1}')

  # Use aria2c for fast download with automatic checksum verification
  # -x16, -s16: use 16 connections for speed
  log_info "Fetching ${remote_img_name} via aria2c..."
  aria2c --checksum=sha-512="${expected_sha}" \
         -x16 -s16 \
         -d "${local_cache_dir}" -o "${remote_img_name}" \
         "${url}"

  log_info "Extracting image to workspace..."
  tar xJ -f "${local_cached_img}" -C "${DEBIAN_BASE_DIR}"
}

# Compile ttyd using a pre-installed musl toolchain
compile_ttyd() {
  local install_path="${CHROOT_TTYD_DIR}/usr/local/bin/ttyd"
  if [[ "$MAY_SKIP_BUILD" == 1 && -f "${install_path}" ]]; then
    log_info "Skipping ttyd build: ${install_path} already exists."
    return
  fi

  log_step "Compiling ttyd terminal proxy..."
  local ttyd_version=1.7.7
  local build_env=(
    "BUILD_TARGET=${TARGET_ARCH}"
    "CROSS_ROOT=/opt/musl-toolchains"
    "STAGE_ROOT=${WORKDIR}/tmp.ttyd/stage"
    "BUILD_ROOT=${WORKDIR}/tmp.ttyd/build"
  )

  cp -r "$SCRIPT_DIR/ttyd/" "${WORKDIR}"
  pushd "${WORKDIR}" > /dev/null
  wget -qO- "https://github.com/tsl0922/ttyd/archive/refs/tags/${ttyd_version}.tar.gz" | tar xz
  cp ttyd/* ttyd-${ttyd_version}/scripts

  pushd "ttyd-${ttyd_version}" > /dev/null
  bash -c "env ${build_env[*]} ./scripts/cross-build.sh"

  mkdir -p "${CHROOT_TTYD_DIR}/usr/local/bin"
  cp "${WORKDIR}/tmp.ttyd/stage/${TARGET_ARCH}-linux-musl/bin/ttyd" "${install_path}"
  chmod 755 "${install_path}"
  popd > /dev/null
  popd > /dev/null
}

# Extract and customize the root filesystem image using chroot
customize_rootfs() {
  if [[ "$MAY_SKIP_BUILD" == 1 && -f "${ROOT_PART_FILE}" ]]; then
    log_info "Skipping rootfs customization: ${ROOT_PART_FILE} already exists."
    return
  fi

  log_step "Extracting and customizing root filesystem..."
  local root_partition_num=1
  local loop_device=$(losetup -f --show --partscan "${RAW_DISK_IMG}")
  dd if="${loop_device}p$root_partition_num" of="${ROOT_PART_FILE}" bs=4M conv=sparse status=progress
  losetup -d "${loop_device}"

  # Run customization logic inside the guest filesystem
  "${SCRIPT_DIR}/chroot_rootfs.sh" \
    -b "${SCRIPT_DIR}:/mnt/build" \
    -b "${CHROOT_TTYD_DIR}:/mnt/ttyd" \
    -b "/mnt/apt-cache:/var/cache/apt/archives" \
    -c "/mnt/build/build_rootfs_in_chroot.sh" \
    "${ROOT_PART_FILE}"

  log_info "Synchronizing filesystem changes..."
  sync
}

# Package the customized filesystem with the kernel and initrd
generate_final_package() {
  log_step "Creating final delivery package: ${FINAL_OUTPUT_FILE}"
  pushd "${WORKDIR}" > /dev/null

  echo "${DEBIAN_BUILD_ID}" > build_id
  cp "$SCRIPT_DIR/vm_config.json" vm_config.json

  # Sync the filesystem UUID with the VM configuration
  local root_uuid=$(sfdisk --part-uuid "${RAW_DISK_IMG}" 1)
  sed -i "s/{root_part_guid}/${root_uuid}/g" vm_config.json

  # Fetch the latest Android Common Kernel from CI if not specified
  if [[ -z "${KERNEL_BUILD_ID}" ]]; then
    KERNEL_BUILD_ID=$(curl -s https://ci.android.com/builds/branches/aosp_kernel-common-android16-6.12/status.json | \
      jq -r '.targets[] | select(.name == "kernel_server_'${TARGET_ARCH}'") | .last_known_good_build')
  fi

  local kernel_url="https://androidbuildinternal.googleapis.com/android/internal/build/v3/builds/${KERNEL_BUILD_ID}/kernel_server_${TARGET_ARCH}/attempts/latest/artifacts"
  wget -q -O vmlinuz "${kernel_url}/${VMLINUZ_NAME}/url"
  wget -q -O initrd.img "${kernel_url}/initramfs.img/url"

  local bundle_contents=(build_id "${ROOT_PART_FILE##*/}" vm_config.json vmlinuz initrd.img)
  popd > /dev/null

  # Compress everything into the final tarball using multi-core pigz
  tar -I pigz -cv -f "${FINAL_OUTPUT_FILE}" -C "${WORKDIR}" "${bundle_contents[@]}"
}

# Delete the temporary work directory unless preservation is requested
cleanup() {
  [[ "$SAVE_WORKDIR" -eq 1 ]] || rm -rf "${WORKDIR}"
}

### --- Main Execution --- ###

main() {
  parse_options "$@"
  initialize_paths

  trap cleanup EXIT

  # Run the build process with internal timestamps
  {
    fetch_debian_image
    compile_ttyd
    customize_rootfs
    generate_final_package
  } 2>&1 | ts -s

  log_step "Build completed successfully: ${FINAL_OUTPUT_FILE}"
}

main "$@"
