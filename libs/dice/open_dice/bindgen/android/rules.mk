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

MODULE_SRCS := $(LOCAL_DIR)/lib.rs

MODULE_CRATE_NAME := open_dice_android_bindgen

MODULE_LIBRARY_DEPS += \
	external/open-dice \
	$(LOCAL_DIR)/../dice \
	trusty/user/base/lib/trusty-sys \

MODULE_BINDGEN_ALLOW_FUNCTIONS := \
	DiceAndroidFormatConfigDescriptor \
	DiceAndroidMainFlow \
	DiceAndroidHandoverParse \

MODULE_BINDGEN_ALLOW_VARS := \
	DICE_ANDROID_CONFIG_.* \

# Prevent DiceInputValues from being generated a second time and
# import it instead from open_dice_cbor_bindgen.
MODULE_BINDGEN_FLAGS += \
	--blocklist-type="DiceInputValues_" \
	--blocklist-type="DiceInputValues" \
	--raw-line \
	"pub use open_dice_cbor_bindgen::DiceInputValues;" \

# Prevent DiceResult from being generated a second time and
# import it instead from open_dice_cbor_bindgen.
MODULE_BINDGEN_FLAGS += \
	--blocklist-type="DiceResult" \
	--raw-line \
	"pub use open_dice_cbor_bindgen::DiceResult;" \

MODULE_BINDGEN_SRC_HEADER := $(LOCAL_DIR)/android.h

include make/library.mk
