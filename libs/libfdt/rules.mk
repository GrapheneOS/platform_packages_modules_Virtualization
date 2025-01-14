# Copyright (C) 2024 The Android Open Source Project
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
#

LOCAL_DIR := $(GET_LOCAL_DIR)

MODULE := $(LOCAL_DIR)

SRC_DIR := packages/modules/Virtualization/libs/libfdt

MODULE_SRCS := $(SRC_DIR)/src/lib.rs

MODULE_CRATE_NAME := libfdt

MODULE_RUST_EDITION := 2021

MODULE_LIBRARY_DEPS += \
	external/dtc/libfdt \
	packages/modules/Virtualization/libs/libfdt/bindgen \
	$(call FIND_CRATE,zerocopy) \
	$(call FIND_CRATE,static_assertions) \

MODULE_RUST_USE_CLIPPY := true

include make/library.mk
