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

use super::idmap::{IdMap, MapError};
use crate::heap::aligned_boxed_slice;
use crate::memory::{SIZE_128KB, SIZE_4KB};
use crate::mmu::{MmuOps, MmuResult};
use core::arch::asm;
use core::arch::x86_64::_mm_mfence;
use core::ops::Range;
use x86_64::addr::VirtAddr;
use x86_64::registers::model_specific::{Efer, EferFlags};
use x86_64::structures::paging::page::PageRangeInclusive;
use x86_64::structures::paging::page_table::{PageTableEntry, PageTableFlags};
use x86_64::structures::paging::{Page, PageSize, Size4KiB};

/// Software bit used to indicate a device that should be lazily mapped.
const MMIO_LAZY_MAP_FLAG: PageTableFlags = PageTableFlags::BIT_9;

const DEVICE_LAZY: PageTableFlags =
    MMIO_LAZY_MAP_FLAG.union(PageTableFlags::WRITABLE).union(PageTableFlags::NO_CACHE);
const DEVICE: PageTableFlags = DEVICE_LAZY.union(PageTableFlags::PRESENT);

const RODATA: PageTableFlags = PageTableFlags::PRESENT;
const DATA: PageTableFlags = RODATA.union(PageTableFlags::WRITABLE);
const CODE: PageTableFlags = PageTableFlags::PRESENT;

fn nxe_supported() -> bool {
    Efer::read().contains(EferFlags::NO_EXECUTE_ENABLE)
}

/// High-level API for managing MMU mappings.
pub struct PageTable {
    idmap: IdMap,
    nx_flag: PageTableFlags,
}

impl Default for PageTable {
    fn default() -> Self {
        Self {
            idmap: IdMap::new(),
            nx_flag: if nxe_supported() {
                PageTableFlags::NO_EXECUTE
            } else {
                PageTableFlags::empty()
            },
        }
    }
}

impl MmuOps for PageTable {
    // # SAFETY
    // Caller should guarantee switching to a different translation is safe.
    unsafe fn activate(&mut self) {
        // SAFETY: the caller of this unsafe function asserts that switching to a different
        // translation is safe.
        unsafe { self.idmap.activate() }
    }

    fn mark_as_lazy_device(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&mut as_page_range(range), DEVICE_LAZY.union(self.nx_flag))?;
        Ok(())
    }

    fn map_device(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&mut as_page_range(range), DEVICE.union(self.nx_flag))?;
        Ok(())
    }

    fn map_device_expect_lazy(&mut self, range: &Range<usize>) -> MmuResult<()> {
        let mut region = as_page_range(range);

        self.idmap.modify_range(&mut region, |pte: &mut PageTableEntry| {
            let flags = pte.flags();
            if !flags.contains(MMIO_LAZY_MAP_FLAG) || flags.contains(PageTableFlags::PRESENT) {
                return Err(MapError::PteUpdateFault(pte.clone()));
            }
            pte.set_flags(PageTableFlags::PRESENT);
            Ok(())
        })?;
        Ok(())
    }

    fn map_data(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&mut as_page_range(range), DATA.union(self.nx_flag))?;
        Ok(())
    }

    fn map_data_track_dirty_state(&mut self, range: &Range<usize>) -> MmuResult<()> {
        // X86 doesn't require dirty stracking in software.
        self.map_data(range)
    }

    fn map_code(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&mut as_page_range(range), CODE)?;
        Ok(())
    }

    fn map_rodata(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.map_range(&mut as_page_range(range), RODATA.union(self.nx_flag))?;
        Ok(())
    }

    fn mark_data_dirty(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        // NOP for x86 as there is no need for dirty tracking in software.
        Ok(())
    }

    fn unmap(&mut self, range: &Range<usize>) -> MmuResult<()> {
        self.idmap.unmap_range(&mut as_page_range(range))?;
        Ok(())
    }

    fn sync_dirty_state(&mut self) -> MmuResult<()> {
        // Memory barrier to ensure all hardware updates to the page table have been
        // observed before accessing PTE.
        // SAFETY: memory barrer instruction
        unsafe {
            _mm_mfence();
        }
        Ok(())
    }

    fn flush_dirty_pages(&mut self, _range: &Range<usize>) -> MmuResult<()> {
        // NOP for x86. No need for selective data cache flush as x86 cache
        // coherency protocols take care of data consistency.
        Ok(())
    }
}

impl Drop for PageTable {
    fn drop(&mut self) {
        // SAFETY: Static PT is untouched and still valid.
        unsafe {
            self.idmap.deactivate();
        }
    }
}

fn as_page_range(range: &Range<usize>) -> PageRangeInclusive<Size4KiB> {
    let start_page = Page::containing_address(VirtAddr::new(range.start as u64));
    let end_page = Page::containing_address(VirtAddr::new((range.end - 1) as u64));
    Page::range_inclusive(start_page, end_page)
}
