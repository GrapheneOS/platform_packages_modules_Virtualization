/*
 * Copyright 2025 The Android Open Source Project
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

//! Get the hw timeout multiplier to adapt VM operation timeouts

use rustutils::android::system_properties;

/// Returns the multiplier that's set in the system properties, or 1 if undefined.
pub fn timeout_multiplier() -> u64 {
    // Android allows setting hardware multipliers for dealing with situations such as running on
    // slower hardware. An example of this is nested virtualization.
    system_properties::read("ro.hw_timeout_multiplier")
        .unwrap_or_else(|_| Some("1".to_string()))
        .unwrap_or_else(|| "1".to_string())
        .parse()
        .unwrap_or(1)
}

/// Returns multiplier squared, capped at 50.
pub fn vm_timeout_multiplier() -> u64 {
    std::cmp::min(timeout_multiplier().pow(2), 50)
}
