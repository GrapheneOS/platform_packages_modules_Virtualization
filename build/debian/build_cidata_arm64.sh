#!/bin/bash

set -ex

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
TOP="${SCRIPT_DIR}/../../../../../"

pushd $TOP

# This is to required to build linux_musl_x86_64
export USE_HOST_MUSL=true

# Some notes:
#   - TARGET_PRODUCT must specify one with HOST_CROSS_OS := linux_musl
#   - target 'dist' is required to enable dist mode.
build/soong/soong_ui.bash --make-mode TARGET_PRODUCT=cf_arm64_only_phone TARGET_RELEASE=trunk_staging TARGET_BUILD_VARIANT=userdebug \
	ferrochrome_dist dist

popd

echo "Done. check out/dist/cidata.iso and out/dist/cidata_x86_64.iso"
