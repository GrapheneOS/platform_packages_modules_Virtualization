// Copyright 2023, The Android Open Source Project
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

//! Vsock setup shared between the host and the service VM.

const PROTECTED_VM_PORT: u32 = 5679;
const NON_PROTECTED_VM_PORT: u32 = 5680;

/// VM Type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmType {
    /// Protected VM.
    ProtectedVm,

    /// NonProtectev VM.
    NonProtectedVm,
}

impl VmType {
    /// Returns the port number used for the vsock communication between
    /// the host and the service VM.
    pub fn port(&self) -> u32 {
        match self {
            Self::ProtectedVm => PROTECTED_VM_PORT,
            Self::NonProtectedVm => NON_PROTECTED_VM_PORT,
        }
    }

    /// Returns whether it is a protected VM.
    pub fn is_protected(&self) -> bool {
        match self {
            Self::ProtectedVm => true,
            Self::NonProtectedVm => false,
        }
    }
}
