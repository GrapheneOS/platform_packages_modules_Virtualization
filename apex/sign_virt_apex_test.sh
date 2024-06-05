#!/usr/bin/env bash

# Copyright (C) 2021 The Android Open Source Project
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#      http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

set -e
shopt -s extglob

TMP_ROOT=$(mktemp -d -t sign_virt_apex-XXXXXXXX)
TEST_DIR=$(dirname $0)

# To access host tools
PATH=$TEST_DIR:$PATH
DEBUGFS=$TEST_DIR/debugfs_static
FSCKEROFS=$TEST_DIR/fsck.erofs

echo "Extracting the virt apex ..."
deapexer --debugfs_path $DEBUGFS --fsckerofs_path $FSCKEROFS \
  extract $TEST_DIR/com.android.virt.apex $TMP_ROOT

if [ "$(ls -A $TMP_ROOT/etc/fs/)" ]; then
  echo "Re-signing the contents ..."
  sign_virt_apex -v $TEST_DIR/test.com.android.virt.pem $TMP_ROOT
  echo "Verifying the contents ..."
  sign_virt_apex -v --verify $TEST_DIR/test.com.android.virt.pem $TMP_ROOT
  echo "Done."
else
  echo "No filesystem images. Skip."
fi

