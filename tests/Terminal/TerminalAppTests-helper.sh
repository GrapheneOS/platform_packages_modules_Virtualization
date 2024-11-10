#!/bin/sh

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

set -e

user=$(am get-current-user)
package_name=$(pm list package virtualization.terminal | cut -d ':' -f 2)

if [ $1 == "setup" ]; then
	pm enable --user ${user} ${package_name}
elif [ $1 == "teardown" ]; then
	pm clear --user ${user} ${package_name}
	pm disable --user ${user} ${package_name}
	rm -rf /data/media/${user}/linux
else
	echo Unsupported command: $1
	exit 1
fi
