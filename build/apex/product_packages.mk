#
# Copyright (C) 2021 Google Inc.
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

# TODO: Remove this once the APEX is included in base system.

# To include the APEX in your build, insert this in your device.mk:
#   $(call inherit-product, packages/modules/Virtualization/build/apex/product_packages.mk)

# If devices supports AVF it implies that it uses non-flattened APEXes.
$(call inherit-product, $(SRC_TARGET_DIR)/product/updatable_apex.mk)

PRODUCT_PACKAGES += \
    com.android.compos \
    features_com.android.virt.xml

# TODO(b/207336449): Figure out how to get these off /system
PRODUCT_ARTIFACT_PATH_REQUIREMENT_ALLOWED_LIST := \
    system/framework/oat/%@service-compos.jar@classes.odex \
    system/framework/oat/%@service-compos.jar@classes.vdex \

PRODUCT_APEX_SYSTEM_SERVER_JARS := com.android.compos:service-compos

PRODUCT_SYSTEM_EXT_PROPERTIES := ro.config.isolated_compilation_enabled=true

PRODUCT_FSVERITY_GENERATE_METADATA := true

PRODUCT_AVF_ENABLED := true

# The cheap build flags dependency management system until there is a proper one.
ifdef RELEASE_AVF_ENABLE_DEVICE_ASSIGNMENT
  ifndef RELEASE_AVF_ENABLE_VENDOR_MODULES
    $(error RELEASE_AVF_ENABLE_VENDOR_MODULES must also be enabled)
  endif
endif

ifdef RELEASE_AVF_ENABLE_LLPVM_CHANGES
  ifndef RELEASE_AVF_ENABLE_DICE_CHANGES
    $(error RELEASE_AVF_ENABLE_DICE_CHANGES must also be enabled)
  endif
endif

ifdef RELEASE_AVF_ENABLE_REMOTE_ATTESTATION
  ifndef RELEASE_AVF_ENABLE_DICE_CHANGES
    $(error RELEASE_AVF_ENABLE_DICE_CHANGES must also be enabled)
  endif
endif

ifdef RELEASE_AVF_ENABLE_NETWORK
  ifndef RELEASE_AVF_ENABLE_LLPVM_CHANGES
    $(error RELEASE_AVF_ENABLE_LLPVM_CHANGES must also be enabled)
  endif
endif

ifdef RELEASE_AVF_ENABLE_EARLY_VM
  # We can't query TARGET_RELEASE from here, so we use RELEASE_AIDL_USE_UNFROZEN as a proxy value of
  # whether we are building -next release.
  ifneq ($(RELEASE_AIDL_USE_UNFROZEN),true)
    $(error RELEASE_AVF_ENABLE_EARLY_VM can only be enabled in trunk_staging until b/357025924 is fixed)
  endif
endif

ifdef RELEASE_AVF_SUPPORT_CUSTOM_VM_WITH_PARAVIRTUALIZED_DEVICES
  PRODUCT_PACKAGES += LinuxInstallerAppStub
endif
