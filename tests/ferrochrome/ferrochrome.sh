#!/bin/bash

# Copyright 2024 Google Inc. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

## Booting tests for ferrochrome
## Keep this file synced with docs/custom_vm.md

set -e

FECR_GS_URL="https://storage.googleapis.com/chromiumos-image-archive/ferrochrome-public"
FECR_DEFAULT_VERSION="R127-15916.0.0"
FECR_DEVICE_DIR="/data/local/tmp/ferrochrome"
FECR_CONFIG_PATH="/data/local/tmp/vm_config.json"  # hardcoded at VmLauncherApp
FECR_CONSOLE_LOG_PATH="/data/data/\${pkg_name}/files/console.log"
FECR_BOOT_COMPLETED_LOG="Have fun and send patches!"
FECR_BOOT_TIMEOUT="300" # 5 minutes (300 seconds)
AOSP_PKG_NAME="com.android.virtualization.vmlauncher"
SIGNED_PKG_NAME="com.google.android.virtualization.vmlauncher"

fecr_clean_up() {
  trap - INT

  if [[ -d ${fecr_dir} && -z ${fecr_keep} ]]; then
    rm -rf ${fecr_dir}
  fi
}

print_usage() {
  echo "ferochrome.sh: Launches ferrochrome image"
  echo ""
  echo "By default, this downloads ferrochrome image with version ${FECR_DEFAULT_VERSION},"
  echo "launches, and waits for boot completed."
  echo "When done, removes downloaded image."
  echo ""
  echo "Usage: ferrochrome.sh [options]"
  echo ""
  echo "Options"
  echo "  --help or -h: This message"
  echo "  --dir \${dir}: Use ferrochrome images at the dir instead of downloading"
  echo "  --skip: Skipping downloading and/or pushing images"
  echo "  --version \${version}: ferrochrome version to be downloaded"
  echo "  --keep: Keep downloaded ferrochrome image"
}


fecr_version=""
fecr_dir=""
fecr_keep=""
fecr_skip=""
fecr_script_path=$(dirname ${0})

# Parse parameters
while (( "${#}" )); do
  case "${1}" in
    --version)
      shift
      fecr_version="${1}"
      ;;
    --dir)
      shift
      fecr_dir="${1}"
      fecr_keep="true"
      ;;
    --keep)
      fecr_keep="true"
      ;;
    --skip)
      fecr_skip="true"
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      print_usage
      exit 1
      ;;
  esac
  shift
done

trap fecr_clean_up INT
trap fecr_clean_up EXIT

if [[ -z "${fecr_skip}" ]]; then
  if [[ -z "${fecr_dir}" ]]; then
    # Download fecr image archive, and extract necessary files
    # DISCLAIMER: Image is too large (1.5G+ for compressed, 6.5G+ for uncompressed), so can't submit.
    fecr_dir=$(mktemp -d)

    echo "Downloading ferrochrome image to ${fecr_dir}"
    fecr_version=${fecr_version:-${FECR_DEFAULT_VERSION}}
    curl --output-dir ${fecr_dir} -O ${FECR_GS_URL}/${fecr_version}/image.zip
  fi
  if [[ ! -f "${fecr_dir}/chromiumos_test_image.bin" ]]; then
    unzip ${fecr_dir}/image.zip chromiumos_test_image.bin boot_images/vmlinuz* -d ${fecr_dir} > /dev/null
  fi

  echo "Pushing ferrochrome image to ${FECR_DEVICE_DIR}"
  adb shell mkdir -p ${FECR_DEVICE_DIR} > /dev/null || true
  adb push ${fecr_dir}/chromiumos_test_image.bin ${FECR_DEVICE_DIR}
  adb push ${fecr_dir}/boot_images/vmlinuz ${FECR_DEVICE_DIR}
  adb push ${fecr_script_path}/assets/vm_config.json ${FECR_CONFIG_PATH}
fi

adb root > /dev/null
adb shell pm list packages | grep ${AOSP_PKG_NAME} > /dev/null
if [[ "${?}" == "0" ]]; then
  pkg_name=${AOSP_PKG_NAME}
else
  pkg_name=${SIGNED_PKG_NAME}
fi

adb shell pm enable ${pkg_name}/${AOSP_PKG_NAME}.MainActivity > /dev/null
adb shell pm grant ${pkg_name} android.permission.USE_CUSTOM_VIRTUAL_MACHINE > /dev/null
adb shell pm clear ${pkg_name} > /dev/null

echo "Starting ferrochrome"
adb shell am start-activity ${pkg_name}/${AOSP_PKG_NAME}.MainActivity > /dev/null

log_path="/data/data/${pkg_name}/files/console.log"
fecr_start_time=${EPOCHSECONDS}

while [[ $((EPOCHSECONDS - fecr_start_time)) -lt ${FECR_BOOT_TIMEOUT} ]]; do
  adb shell grep -sF \""${FECR_BOOT_COMPLETED_LOG}"\" "${log_path}" && exit 0
  sleep 10
done

echo "Ferrochrome failed to boot"
exit 1
