/*
 * Copyright 2022 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Detection for nested virtualization.

use anyhow::Result;
use rustutils::system_properties;

/// Return whether we will be running our VM in a VM, which causes the nested VM to run very slowly.
pub fn is_nested_virtualization() -> Result<bool> {
    // Nested virtualization occurs when we run KVM inside the cuttlefish VM or when
    // we run trusty within qemu.
    let checks = [
        ("ro.product.vendor.device", "vsoc_"), // vsoc_x86, vsoc_x86_64, vsoc_x86_64_only, ...
        ("ro.hardware", "qemu_"),              // qemu_trusty, ...
    ];

    for (property, prefix) in checks {
        if let Some(value) = system_properties::read(property)? {
            if value.starts_with(prefix) {
                return Ok(true);
            }
        }
    }

    // No match -> not nested
    Ok(false)
}
