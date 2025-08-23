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

//! Page table management.

use crate::mmu::{MmuOps, MmuResult};
use core::ops::Range;

/// High-level API for managing MMU mappings.
#[derive(Default)]
pub struct PageTable {}

impl Drop for PageTable {
    fn drop(&mut self) {
        // TODO(b/354116267): implement for x86_64
    }
}

impl MmuOps for PageTable {
    unsafe fn activate(&mut self) {}

    fn mark_as_lazy_device(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        Ok(())
    }

    fn map_device(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        Ok(())
    }

    fn map_device_expect_lazy(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        // TODO(b/362733888): Provide the implementation for x86_64
        Ok(())
    }

    fn map_data(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        Ok(())
    }

    fn map_data_track_dirty_state(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        Ok(())
    }

    fn map_code(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        Ok(())
    }

    fn map_rodata(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        Ok(())
    }

    fn mark_data_dirty(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        Ok(())
    }

    fn sync_dirty_state(&mut self) -> MmuResult<()> {
        Ok(())
    }

    fn flush_dirty_pages(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        // TODO(b/362733888): Provide the implementation for x86_64
        Ok(())
    }
}
