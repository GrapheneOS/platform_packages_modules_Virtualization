// Copyright 2025, The Android Open Source Project
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

//! Contains request and response definitions between this VM and the host.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// Port VM listens to requests on.
pub const VM_PORT: u32 = 17239;

/// Request sent to the VM.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Request {
    /// Reverses the provided array and sends it back to host.
    Reverse(Vec<u8>),
    /// Maps data at the given range.
    MapData(usize, usize),
    /// Relinquishes given range of pages.
    MemRelinquish(usize, usize),
    /// Reads the mapped data at the given range.
    ReadMappedData(usize, usize),
    /// Shut down the VM. No response is expected.
    Shutdown,
}

/// Response received from the VM.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    /// Response to the `Request::Reverse`.
    Reverse(Vec<u8>),
    /// Response to the `Request::MapData`.
    /// Contains `true` if map data succeeds, or `false` otherwise. If map data succeeds then you
    /// can use `Request::ReadMappedData` to read the mapped data and return it back to host.
    MapData(bool),
    /// Response to the `Request::MemRelinquish`.
    /// Contains `true` if request was handled successfully, or `false` otherwise.
    MemRelinquish(bool),
    /// Response to the `Request::ReadMappedData`.
    ReadMappedData(Vec<u8>),
}
