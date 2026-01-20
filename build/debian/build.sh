#!/bin/bash
#
# ==============================================================================
# Ferrochrome Guest Image Build Script
# ==============================================================================
#
# This script is the entry point for building a Debian-based guest image for
# the Android Virtualization Framework (AVF).
#
# It performs the following steps:
# 1. Configures the host environment for cross-architecture builds (QEMU).
# 2. Builds a dedicated Docker builder image (ferrochrome-builder).
# 3. Runs the build process inside a privileged container with cached resources.
#
# Usage: sudo ./build.sh [OPTIONS]
# ==============================================================================

set -eo pipefail

### --- Configuration & Defaults --- ###

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
BUILDER_IMAGE_NAME="ferrochrome-builder"
REPO_ROOT_DIR="${SCRIPT_DIR}/../../"

# Default build parameters
TARGET_ARCH="$(uname -m)"
DEBIAN_BUILD_ID="eng-1000000-$(date --utc +'%a %b %d %H:%M:%S %Z %Y')"
DOCKER_BASE_IMAGE="ubuntu:22.04"

# Command-line argument flags
KERNEL_BUILD_ID_ARGS=""
SAVE_WORKDIR_ARGS=""
POST_BUILD_SHELL="|| bash"
CUSTOM_WORKDIR_MOUNT=""
INTERNAL_WORKDIR_ARGS=""

### --- Utilities --- ###

# Log informational messages in green
log() { echo -e "\033[1;32m[INFO]\033[0m $1"; }

# Log error messages in red and exit
error() { echo -e "\033[1;31m[ERROR]\033[0m $1" >&2; exit 1; }

# Print help message
show_help() {
  echo "Usage: $0 [OPTION]..."
  echo "Builds images.tar.gz with Debian payload."
  echo ""
  echo "Options:"
  echo "-a TARGET_ARCH        Architecture of the image [default: $TARGET_ARCH]"
  echo "-b DEBIAN_BUILD_ID    Set build id of the debian image"
  echo "-k KERNEL_ID          Build ID for kernel [default: last known good]"
  echo "-h                    Print usage and this help message and exit."
  echo "-i DOCKER_BASE_IMAGE  Specify the builder's base image [default: $DOCKER_BASE_IMAGE]"
  echo "-s                    Leave a shell open if able [default: only if the build fails]"
  echo "-t REPO_ROOT_DIR      Specify the virtualization repo top [default: $REPO_ROOT_DIR]"
  echo "-w                    Save temp work directory in the container [for debugging]"
  echo "-W WORK_DIR           Specify work dir instead of temporarily creating one."
}

### --- Functions --- ###

# Parse command line options
parse_options() {
  while getopts "a:b:k:hi:st:wW:" option; do
    case ${option} in
      a) TARGET_ARCH="$OPTARG" ;;
      b) DEBIAN_BUILD_ID="$OPTARG" ;;
      k) KERNEL_BUILD_ID_ARGS="-k ${OPTARG}" ;;
      h) show_help ; exit ;;
      i) DOCKER_BASE_IMAGE="$OPTARG" ;;
      s) POST_BUILD_SHELL="; bash" ;;
      t) REPO_ROOT_DIR="$OPTARG" ;;
      w) SAVE_WORKDIR_ARGS="-w" ;;
      W)
        CUSTOM_WORKDIR_MOUNT="-v ${OPTARG}:${OPTARG}"
        INTERNAL_WORKDIR_ARGS="-W ${OPTARG}"
        ;;
      *) error "Invalid option: $OPTARG" ;;
    esac
  done

  [[ "$TARGET_ARCH" != "aarch64" && "$TARGET_ARCH" != "x86_64" ]] && error "Invalid architecture: $TARGET_ARCH"

  DOCKER_INTERACTIVE_FLAGS=""
  if [[ -t 0 ]]; then
    DOCKER_INTERACTIVE_FLAGS="-it"
  else
    POST_BUILD_SHELL=""
  fi
}

# Ensure binfmt_misc is configured for cross-architecture builds
ensure_binfmt_misc() {
  if [[ "$TARGET_ARCH" != "$(uname -m)" ]]; then
    if [[ ! -f "/proc/sys/fs/binfmt_misc/qemu-${TARGET_ARCH}" ]]; then
      log "Enabling multi-arch container by QEMU for $TARGET_ARCH..."
      docker run --rm --privileged multiarch/qemu-user-static --reset -p yes
    fi
  fi
}

# Build the dedicated Docker builder image
build_builder() {
  log "Ensuring builder image '$BUILDER_IMAGE_NAME' is up to date..."
  docker build -t "$BUILDER_IMAGE_NAME" "$SCRIPT_DIR"
}

# Run the build process inside the Docker container
run_builder() {
  log "Starting build in container (arch: $TARGET_ARCH)..."

  # Ensure cache directories exist on host to speed up downloads/compilation
  mkdir -p "$HOME/.cache/ferrochrome-apt"
  mkdir -p "$HOME/.cache/ferrochrome-images"

  docker run --privileged --init $DOCKER_INTERACTIVE_FLAGS \
    $CUSTOM_WORKDIR_MOUNT \
    -v /dev:/dev \
    -v "$REPO_ROOT_DIR:/root/Virtualization" \
    -v "$HOME/.cargo/registry:/root/.cargo/registry" \
    -v "$HOME/.cargo/git:/root/.cargo/git" \
    -v "$HOME/.cache/ferrochrome-apt:/mnt/apt-cache" \
    -v "$HOME/.cache/ferrochrome-images:/mnt/image-cache" \
    --workdir /root/Virtualization/build/debian \
    "$BUILDER_IMAGE_NAME" \
    bash -c "set -o pipefail; \
             bash ./build_internal.sh \
                  -a $TARGET_ARCH \
                  $SAVE_WORKDIR_ARGS \
                  $INTERNAL_WORKDIR_ARGS \
                  -b \"$DEBIAN_BUILD_ID\" \
                  $KERNEL_BUILD_ID_ARGS 2>&1 \
             $POST_BUILD_SHELL" | ts -s
}

### --- Main Execution --- ###

parse_options "$@"
ensure_binfmt_misc
build_builder
run_builder
