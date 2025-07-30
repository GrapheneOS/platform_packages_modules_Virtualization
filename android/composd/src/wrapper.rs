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

//! Wraps lower level APIs so they can be mocked for unit tests.

#[cfg_attr(test, mockall::automock)]
pub mod paths {
    use std::path::{Path, PathBuf};
    pub fn get_root() -> PathBuf {
        Path::new("/").to_path_buf()
    }
}
