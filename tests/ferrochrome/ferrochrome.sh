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
FECR_DEVICE_DIR="/data/local/tmp"
FECR_CONFIG_PATH="/data/local/tmp/vm_config.json"  # hardcoded at VmLauncherApp
FECR_CONSOLE_LOG_PATH="/data/data/\${pkg_name}/files/console.log"
FECR_BOOT_COMPLETED_LOG="Have fun and send patches!"
FECR_BOOT_TIMEOUT="300" # 5 minutes (300 seconds)
ACTION_NAME="android.virtualization.VM_LAUNCHER"

fecr_clean_up() {
  trap - INT

  if [[ -d ${fecr_dir} && -z ${fecr_keep} ]]; then
    rm -rf ${fecr_dir}
  fi
}

print_usage() {
  echo "ferochrome: Launches ferrochrome image"
  echo ""
  echo "By default, this downloads ferrochrome image with version ${FECR_DEFAULT_VERSION},"
  echo "launches, and waits for boot completed."
  echo "When done, removes downloaded image on host while keeping pushed image on device."
  echo ""
  echo "Usage: ferrochrome [options]"
  echo ""
  echo "Options"
  echo "  --help or -h: This message"
  echo "  --dir DIR: Use ferrochrome images at the dir instead of downloading"
  echo "  --verbose: Verbose log message (set -x)"
  echo "  --skip: Skipping downloading and/or pushing images"
  echo "  --version \${version}: ferrochrome version to be downloaded"
  echo "  --keep: Keep downloaded ferrochrome image"
}

fecr_version=""
fecr_dir=""
fecr_keep=""
fecr_skip=""
fecr_script_path=$(dirname ${0})
fecr_verbose=""

# Parse parameters
while (( "${#}" )); do
  case "${1}" in
    --verbose)
      fecr_verbose="true"
      ;;
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

if [[ -n "${fecr_verbose}" ]]; then
  set -x
fi

. "${fecr_script_path}/ferrochrome-precondition-checker.sh"

resolved_activities=$(adb shell pm query-activities --components -a ${ACTION_NAME})

if [[ "$(echo ${resolved_activities} | wc -l)" != "1" ]]; then
  >&2 echo "Multiple VM launchers exists"
  exit 1
fi

pkg_name=$(dirname ${resolved_activities})

adb shell pm grant ${pkg_name} android.permission.USE_CUSTOM_VIRTUAL_MACHINE > /dev/null
adb shell pm clear ${pkg_name} > /dev/null

if [[ -z "${fecr_skip}" ]]; then
  if [[ -z "${fecr_dir}" ]]; then
    # Download fecr image archive, and extract necessary files
    # DISCLAIMER: Image is too large (1.5G+ for compressed, 6.5G+ for uncompressed), so can't submit.
    fecr_dir=$(mktemp -d)

    echo "Downloading & extracting ferrochrome image to ${fecr_dir}"
    fecr_version=${fecr_version:-${FECR_DEFAULT_VERSION}}
    curl ${FECR_GS_URL}/${fecr_version}/chromiumos_test_image.tar.xz | tar xfJ - -C ${fecr_dir}
  fi

  echo "Pushing ferrochrome image to ${FECR_DEVICE_DIR}"
  adb shell mkdir -p ${FECR_DEVICE_DIR} > /dev/null || true
  adb push ${fecr_dir}/chromiumos_test_image.bin ${FECR_DEVICE_DIR}
  adb push ${fecr_script_path}/assets/vm_config.json ${FECR_CONFIG_PATH}
fi

echo "Starting ferrochrome"
adb shell am start-activity -a ${ACTION_NAME} > /dev/null

if [[ $(adb shell getprop ro.fw.mu.headless_system_user) == "true" ]]; then
  current_user=$(adb shell am get-current-user)
  log_path="/data/user/${current_user}/${pkg_name}/files/console.log"
else
  log_path="/data/data/${pkg_name}/files/console.log"
fi
fecr_start_time=${EPOCHSECONDS}

while [[ $((EPOCHSECONDS - fecr_start_time)) -lt ${FECR_BOOT_TIMEOUT} ]]; do
  adb shell grep -sF \""${FECR_BOOT_COMPLETED_LOG}"\" "${log_path}" && exit 0
  sleep 10
done

>&2 echo "Ferrochrome failed to boot. Dumping console log"
>&2 adb shell cat ${log_path}

exit 1
