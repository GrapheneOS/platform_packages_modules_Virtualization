// Copyright 2022, The Android Open Source Project
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

//! Native helper for compos_verify to call boringssl.

pub use native::*;

#[cxx::bridge]
mod native {
    unsafe extern "C++" {
        include!("verify_native.h");

        // SAFETY: The C++ implementation manages its own memory, and does not retain or abuse
        // the references passed to it.

        /// Verify a PureEd25519 signature with the specified public key on the given data,
        /// returning whether the signature is valid or not.
        fn verify(public_key: &[u8], signature: &[u8], data: &[u8]) -> bool;
    }
}
