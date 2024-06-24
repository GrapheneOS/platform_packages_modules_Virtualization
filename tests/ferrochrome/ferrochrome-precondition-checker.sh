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


## Precondition checks for running ferrochrome
## Used by CI for skipping tests.

REQUIRED_DISK_SPACE=7340032    # Requires 7G, while image is 6.5G

# `adb root` always returns exit code 0
if [[ "$(adb root)" == *"cannot"* ]]; then
  >&2 echo "Failed to run adb root"
  exit 1
fi

# `pm resolve-activity` always returns exit code 0
resolved_activity=$(adb shell pm resolve-activity -a android.virtualization.VM_LAUNCHER)
if [[ "${resolved_activity}" == "No activity found" ]]; then
  >&2 echo "Failed to find vmlauncher"
  exit 1
fi

free_space=$(adb shell df /data/local | tail -1 | awk '{print $4}')
if [[ ${free_space} -lt ${REQUIRED_DISK_SPACE} ]]; then
  >&2 echo "Insufficient space on DUT. Need ${REQUIRED_DISK_SPACE}, but was ${free_space}"
  exit 1
fi

free_space=$(df /tmp | tail -1 | awk '{print $4}')
if [[ ${free_space} -lt ${REQUIRED_DISK_SPACE} ]]; then
  >&2 echo "Insufficient space on host. Need ${REQUIRED_DISK_SPACE}, but was ${free_space}"
  exit 1
fi

cpu_abi=$(adb shell getprop ro.product.cpu.abi)
if [[ "${cpu_abi}" != "arm64"* ]]; then
  >&2 echo "Unsupported architecture. Requires arm64, but was ${cpu_abi}"
  exit 1
fi

device=$(adb shell getprop ro.product.vendor.device)
if [[ "${device}" == "vsock_"* ]]; then
  >&2 echo "Unsupported device. Cuttlefish isn't supported"
  exit 1
fi
