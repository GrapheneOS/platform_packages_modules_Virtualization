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

//! This crate provides methods that are used for CompOS.

#[cfg_attr(test, mockall::automock)]
/// Sets up the binder interface that will be used to test the CompOS API.
pub mod binder {
    use android_system_composd::aidl::android::system::composd::IIsolatedCompilationService::IIsolatedCompilationService;
    use anyhow::{Error, Result};
    use binder::Strong;

    /// Waits for the composd interface to be ready.
    pub fn wait_for_composd_interface(
        composd_service_name: &str,
    ) -> Result<Strong<dyn IIsolatedCompilationService>> {
        match binder::wait_for_interface(composd_service_name) {
            Ok(svc) => Ok(svc),
            Err(e) => Err(Error::msg(e.to_string())),
        }
    }
}

/// Counts the number of unescaped '!' characters in a string.
///
/// A backslash '\' escapes the next character. Only '!' not preceded by a '\' are counted.
// Returns the number of unescaped `!` in a string.
pub fn count_placeholders(fmt_str: &str) -> u32 {
    let mut placeholder_count = 0;
    let mut escaped = false;
    for c in fmt_str.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
        } else if c == '!' {
            placeholder_count += 1;
        }
    }
    placeholder_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_placeholders() {
        assert_eq!(count_placeholders("hello world"), 0);
        assert_eq!(count_placeholders("hello world!"), 1);
        assert_eq!(count_placeholders("!hello world"), 1);
        assert_eq!(count_placeholders("hello ! world"), 1);
        assert_eq!(count_placeholders("!!!"), 3);
        assert_eq!(count_placeholders("\\!"), 0);
        assert_eq!(count_placeholders("\\!hello world"), 0);
        assert_eq!(count_placeholders("hello\\! world"), 0);
        assert_eq!(count_placeholders("hello world\\!"), 0);
        // Escaped backslash, then placeholder.
        assert_eq!(count_placeholders("\\\\!"), 1);
        assert_eq!(count_placeholders("!\\!"), 1);
        assert_eq!(count_placeholders("\\!!"), 1);
        assert_eq!(count_placeholders("a!b\\!c!!d"), 3);
    }
}
