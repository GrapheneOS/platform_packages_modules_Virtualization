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
FECR_DEFAULT_VERSION="R128-15958.0.0"
FECR_DEFAULT_SCREENSHOT_DIR="/data/local/tmp/ferrochrome_screenshots"  # Hardcoded at AndroidTest.xml
FECR_TEST_IMAGE="chromiumos_test_image"
FECR_BASE_IMAGE="chromiumos_base_image"
FECR_DEVICE_DIR="/data/local/tmp"
FECR_IMAGE_VM_CONFIG_JSON="chromiumos_base_image.bin"  # hardcoded at vm_config.json
FECR_CONFIG_PATH="/data/local/tmp/vm_config.json"  # hardcoded at VmLauncherApp
FECR_CONSOLE_LOG_PATH="files/cros.log" # log file name is ${vm_name}.log
FECR_TEST_IMAGE_BOOT_COMPLETED_LOG="Have fun and send patches!"
FECR_BASE_IMAGE_BOOT_COMPLETED_LOG="Chrome started, our work is done, exiting"
FECR_BOOT_TIMEOUT="300" # 5 minutes (300 seconds)
ACTION_NAME="android.virtualization.VM_LAUNCHER"

# Match this with AndroidTest.xml and assets/vm_config.json
FECR_DEFAULT_IMAGE="${FECR_BASE_IMAGE}"
FECR_DEFAULT_BOOT_COMPLETED_LOG="${FECR_BASE_IMAGE_BOOT_COMPLETED_LOG}"

fecr_clean_up() {
  trap - INT

  # Reset screen always on
  adb shell svc power stayon false

  if [[ -d ${fecr_dir} && -z ${fecr_keep} ]]; then
    rm -rf ${fecr_dir}
  fi
}

print_usage() {
  echo "ferochrome: Launches ferrochrome image"
  echo ""
  echo "By default, this downloads ${FECR_DEFAULT_VERSION} with version ${FECR_DEFAULT_VERSION},"
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
  echo "  --test: Download test image instead"
  echo "  --forever: Keep ferrochrome running forever. Used for manual test"
}

fecr_version="${FECR_DEFAULT_VERSION}"
fecr_dir=""
fecr_keep=""
fecr_skip=""
fecr_script_path=$(dirname ${0})
fecr_verbose=""
fecr_image="${FECR_DEFAULT_IMAGE}"
fecr_boot_completed_log="${FECR_DEFAULT_BOOT_COMPLETED_LOG}"
fecr_screenshot_dir="${FECR_DEFAULT_SCREENSHOT_DIR}"
fecr_forever=""

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
    --test)
      fecr_image="${FECR_TEST_IMAGE}"
      fecr_boot_completed_log="${FECR_TEST_IMAGE_BOOT_COMPLETED_LOG}"
      ;;
    --forever)
      fecr_forever="true"
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
current_user=$(adb shell am get-current-user)

echo "Reset app & granting permission"
adb shell pm clear --user ${current_user} ${pkg_name} > /dev/null
adb shell pm grant --user ${current_user} ${pkg_name} android.permission.RECORD_AUDIO
adb shell pm grant --user ${current_user} ${pkg_name} android.permission.USE_CUSTOM_VIRTUAL_MACHINE > /dev/null

if [[ -z "${fecr_skip}" ]]; then
  if [[ -z "${fecr_dir}" ]]; then
    # Download fecr image archive, and extract necessary files
    # DISCLAIMER: Image is too large (1.5G+ for compressed, 6.5G+ for uncompressed), so can't submit.
    fecr_dir=$(mktemp -d)

    echo "Downloading & extracting ferrochrome image to ${fecr_dir}"
    curl ${FECR_GS_URL}/${fecr_version}/${fecr_image}.tar.xz | tar xfJ - -C ${fecr_dir}
  fi

  echo "Pushing ferrochrome image to ${FECR_DEVICE_DIR}"
  adb shell mkdir -p ${FECR_DEVICE_DIR} > /dev/null || true
  adb push ${fecr_dir}/${fecr_image}.bin ${FECR_DEVICE_DIR}/${FECR_IMAGE_VM_CONFIG_JSON}
  adb push ${fecr_script_path}/assets/vm_config.json ${FECR_CONFIG_PATH}
fi

echo "Ensure screen unlocked"
adb shell svc power stayon true
adb shell wm dismiss-keyguard

echo "Starting ferrochrome"
adb shell am start-activity -a ${ACTION_NAME} > /dev/null

# HSUM aware log path
log_path="/data/user/${current_user}/${pkg_name}/${FECR_CONSOLE_LOG_PATH}"
fecr_start_time=${EPOCHSECONDS}

echo "Check ${log_path} on device for console log"

if [[ "${fecr_forever}" == "true" ]]; then
  echo "Ctrl+C to stop running"
  echo "To open interactive serial console, use following command:"
  echo "adb shell -t /apex/com.android.virt/bin/vm console"
else
  adb shell mkdir -p "${fecr_screenshot_dir}"
  while [[ $((EPOCHSECONDS - fecr_start_time)) -lt ${FECR_BOOT_TIMEOUT} ]]; do
    adb shell screencap -p "${fecr_screenshot_dir}/screenshot-${EPOCHSECONDS}.png"
    adb shell grep -soF \""${fecr_boot_completed_log}"\" "${log_path}" && exit 0 || true
    sleep 10
  done

  >&2 echo "Ferrochrome failed to boot. Dumping console log"
  >&2 adb shell cat ${log_path}

  exit 1
fi

