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

//! This module contains the error thrown by the payload verification API
//! and other errors used in the library.

use avb::{DescriptorError, SlotVerifyError};
use core::fmt;

/// Wrapper around `SlotVerifyError` to add custom pvmfw errors.
/// It is the error thrown by the payload verification API `verify_payload()`.
#[derive(Debug, PartialEq, Eq)]
pub enum PvmfwVerifyError {
    /// Passthrough `SlotVerifyError` with no `SlotVerifyData`.
    AvbError(SlotVerifyError<'static>),
    /// VBMeta has invalid descriptors.
    InvalidDescriptors(DescriptorError),
    /// Unknown vbmeta property.
    UnknownVbmetaProperty,
}

impl From<SlotVerifyError<'_>> for PvmfwVerifyError {
    fn from(error: SlotVerifyError) -> Self {
        // We don't use verification data on failure, drop it to get a `'static` lifetime.
        Self::AvbError(error.without_verify_data())
    }
}

impl From<DescriptorError> for PvmfwVerifyError {
    fn from(error: DescriptorError) -> Self {
        Self::InvalidDescriptors(error)
    }
}

impl fmt::Display for PvmfwVerifyError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::AvbError(e) => write!(f, "{}", e),
            Self::InvalidDescriptors(e) => {
                write!(f, "VBMeta has invalid descriptors. Error: {:?}", e)
            }
            Self::UnknownVbmetaProperty => write!(f, "Unknown vbmeta property"),
        }
    }
}
