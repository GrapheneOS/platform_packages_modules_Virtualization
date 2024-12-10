// Copyright 2024, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Low-level stack support.

/// Configures the maximum size of the stack.
#[macro_export]
macro_rules! limit_stack_size {
    ($len:expr) => {
        #[export_name = "vmbase_stack_limit_client"]
        fn __vmbase_stack_limit_client() -> Option<usize> {
            Some($len)
        }
    };
}

pub(crate) fn max_stack_size() -> Option<usize> {
    extern "Rust" {
        fn vmbase_stack_limit() -> Option<usize>;
    }
    // SAFETY: This function is safe to call as the linker script aliases it to either:
    // - the safe vmbase_stack_limit_default();
    // - the safe vmbase_stack_limit_client() potentially defined using limit_stack_size!()
    unsafe { vmbase_stack_limit() }
}

#[no_mangle]
fn vmbase_stack_limit_default() -> Option<usize> {
    None
}
