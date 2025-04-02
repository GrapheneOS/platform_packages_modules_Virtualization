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

//! Hardware management of the access flag and dirty state.
//! Currently only holds stubs for x86_64.
//! TODO(b/354116267): Provide the proper implementation

use crate::arch::x86_64::paging::{Descriptor, MemoryRegion};
use log::warn;

/// Sets whether the hardware management of access and dirty state is enabled with
/// the given boolean.
pub fn set_dbm_enabled(_enabled: bool) {
    log::warn!("set_dbm_enabled is not yet implemented");
}

/// Flushes a memory range the descriptor refers to, if the descriptor is in writable-dirty state.
#[allow(clippy::result_unit_err)]
pub fn flush_dirty_range(
    _va_range: &MemoryRegion,
    _desc: &Descriptor,
    _level: usize,
) -> Result<(), ()> {
    log::warn!("flush_dirty_range is not yet implemented");
    Ok(())
}

/// Clears read-only flag on a PTE, making it writable-dirty. Used when dirty state is managed
/// in software to handle permission faults on read-only descriptors.
#[allow(clippy::result_unit_err)]
pub fn mark_dirty_block(
    _va_range: &MemoryRegion,
    _desc: &mut Descriptor,
    _level: usize,
) -> Result<(), ()> {
    log::warn!("mark_dirty_block is not yet implemented");
    Ok(())
}
