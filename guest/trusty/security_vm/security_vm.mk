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


ifeq ($(findstring enabled, $(TRUSTY_SYSTEM_VM)),enabled)
PRODUCT_PACKAGES += \
	trusty_security_vm.elf \
	trusty_security_vm_launcher \
	trusty_security_vm_launcher.rc \
	early_vms.xml \

endif
