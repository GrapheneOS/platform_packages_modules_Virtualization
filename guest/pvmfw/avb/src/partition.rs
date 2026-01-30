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

//! Struct and functions relating to well-known partition names.

use avb::IoError;
use core::ffi::CStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PartitionName {
    /// The default `PartitionName` is needed to build the default `HashDescriptor`.
    #[default]
    Kernel,
    InitrdNormal,
    InitrdDebug,
}

impl PartitionName {
    pub(crate) fn new_from_bytes(bytes: &[u8]) -> Option<Self> {
        match bytes {
            x if x == Self::Kernel.as_c_str().to_bytes() => Some(Self::Kernel),
            x if x == Self::InitrdNormal.as_c_str().to_bytes() => Some(Self::InitrdNormal),
            x if x == Self::InitrdDebug.as_c_str().to_bytes() => Some(Self::InitrdDebug),
            _ => None,
        }
    }

    pub(crate) fn as_c_str(&self) -> &'static CStr {
        match self {
            Self::Kernel => c"boot",
            Self::InitrdNormal => c"initrd_normal",
            Self::InitrdDebug => c"initrd_debug",
        }
    }
}

impl TryFrom<&CStr> for PartitionName {
    type Error = IoError;

    fn try_from(partition_name: &CStr) -> Result<Self, Self::Error> {
        Self::new_from_bytes(partition_name.to_bytes()).ok_or(IoError::NoSuchPartition)
    }
}

impl TryFrom<&[u8]> for PartitionName {
    type Error = IoError;

    fn try_from(non_null_terminated_name: &[u8]) -> Result<Self, Self::Error> {
        Self::new_from_bytes(non_null_terminated_name).ok_or(IoError::NoSuchPartition)
    }
}
