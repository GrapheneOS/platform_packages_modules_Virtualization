# Copyright (C) 2025 The Android Open Source Project
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

# This is the default set of packages to load the trusty system vm.
# It can be overridden by device-specific configuration.
TRUSTY_SYSTEM_VM_PRODUCT_PACKAGES ?= trusty_security_vm.elf \
	trusty_security_vm_launcher \
	trusty_security_vm_launcher.rc \
	trusty_security_vm_instance_id \
	trusty_security_vm_rpc_services-base.json \
	early_vms.xml \

ifeq ($(TRUSTY_SYSTEM_VM), enabled_with_placeholder_trusted_hal)
TRUSTY_SYSTEM_VM_PRODUCT_PACKAGES += trusty_security_vm_rpc_services-with_placeholders_thal.json
endif

PRODUCT_PACKAGES += \
	keymint_provisioning_tool \
	$(TRUSTY_SYSTEM_VM_PRODUCT_PACKAGES) \
