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

//! wrapper is a module to enable unit testing by wrapping lower level functions in a mockable
//! module, struct or trait.

#[cfg_attr(test, mockall::automock)]
pub mod composd_native {
    use anyhow::Result;
    use std::path::Path;

    /// Wraps composd_native::palette_create_odrefresh_staging_directory().
    pub fn palette_create_odrefresh_staging_directory() -> Result<&'static Path> {
        composd_native::palette_create_odrefresh_staging_directory()
    }
}

pub mod binder {
    pub struct LazyServiceGuard {
        _lazy_service_guard: binder::LazyServiceGuard,
    }

    impl LazyServiceGuard {
        pub fn new() -> Self {
            Self { _lazy_service_guard: binder::LazyServiceGuard::new() }
        }
    }
    impl Drop for LazyServiceGuard {
        fn drop(&mut self) {}
    }
    impl Clone for LazyServiceGuard {
        fn clone(&self) -> Self {
            Self::new()
        }
    }
    #[cfg(test)]
    mockall::mock! {
        pub LazyServiceGuard{
            pub fn new()-> Self;
        }
        impl Drop for LazyServiceGuard{
            fn drop(&mut self);
        }
        impl Clone for LazyServiceGuard{
            fn clone(&self) -> Self;
        }
    }
}

pub mod compos_common_injection {
    #[cfg(not(test))]
    pub use compos_common::*;
    #[cfg(test)]
    pub use compos_common_with_mocks::*;
}
