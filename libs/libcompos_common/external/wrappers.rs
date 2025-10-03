/*
 * Copyright (C) 2025 The Android Open Source Project
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

#[cfg_attr(test, mockall::automock)]
pub mod binder {
    use android_system_composd::aidl::android::system::composd::IIsolatedCompilationService::IIsolatedCompilationService;
    use anyhow::{Error, Result};
    use binder::Strong;

    pub fn wait_for_composd_interface(
        composd_service_name: &str,
    ) -> Result<Strong<dyn IIsolatedCompilationService>> {
        match binder::wait_for_interface(composd_service_name) {
            Ok(svc) => Ok(svc),
            Err(e) => Err(Error::msg(e.to_string())),
        }
    }
}
