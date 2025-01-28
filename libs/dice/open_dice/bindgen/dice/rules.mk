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

MODULE_CRATE_NAME := open_dice_cbor_bindgen

MODULE_LIBRARY_DEPS += \
	external/open-dice \
	trusty/user/base/lib/trusty-sys \

MODULE_BINDGEN_FLAGS += \
	--rustified-enum DiceConfigType \
	--rustified-enum DiceMode \
	--rustified-enum DiceResult \
	--rustified-enum DicePrincipal \

MODULE_BINDGEN_ALLOW_FUNCTIONS := \
	DiceDeriveCdiPrivateKeySeed \
	DiceDeriveCdiCertificateId \
	DiceMainFlow \
	DiceHash \
	DiceKdf \
	DiceKeypairFromSeed \
	DiceSign \
	DiceCoseSignAndEncodeSign1 \
	DiceVerify \
	DiceGenerateCertificate \

MODULE_BINDGEN_ALLOW_VARS := \
	DICE_CDI_SIZE \
	DICE_HASH_SIZE \
	DICE_HIDDEN_SIZE \
	DICE_INLINE_CONFIG_SIZE \
	DICE_PRIVATE_KEY_SEED_SIZE \
	DICE_ID_SIZE \
	DICE_PRIVATE_KEY_BUFFER_SIZE \

MODULE_BINDGEN_SRC_HEADER := $(LOCAL_DIR)/dice.h

include make/library.mk
