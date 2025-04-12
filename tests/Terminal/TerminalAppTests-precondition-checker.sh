#!/bin/bash

# Copyright 2025 Google Inc. All rights reserved.
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


## Precondition checks for running terminal app
## Used by CI for skipping tests.

COMPRESSED_IMAGE_SIZE=716800    # 700MB
INSTALLED_IMAGE_SIZE=8388608    # 8GB

free_space=$(adb shell df /data | tail -1 | awk '{print $4}')
if [[ ${free_space} -lt ${INSTALLED_IMAGE_SIZE} ]]; then
  >&2 echo "Insufficient space on DUT. Need ${INSTALLED_IMAGE_SIZE}, but was ${free_space}"
  exit 1
fi

free_space=$(df /tmp | tail -1 | awk '{print $4}')
if [[ ${free_space} -lt ${COMPRESSED_IMAGE_SIZE} ]]; then
  >&2 echo "Insufficient space on host. Need ${COMPRESSED_IMAGE_SIZE}, but was ${free_space}"
  exit 1
fi

