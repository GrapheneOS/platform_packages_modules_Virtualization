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

//! Identity mapped Page Table implementation

use alloc::{boxed::Box, collections::BTreeMap};
use thiserror::Error;
use x86_64::instructions::tlb::flush as tlb_flush;
use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::structures::paging::page::PageRangeInclusive;
use x86_64::structures::paging::page_table::{
    PageTable, PageTableEntry, PageTableIndex, PageTableLevel,
};
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

const INVALID_FLAGS: PageTableFlags =
    PageTableFlags::HUGE_PAGE.union(PageTableFlags::ACCESSED).union(PageTableFlags::DIRTY);

/// An error attempting to map some range in the page table.
#[derive(Clone, Debug, Error)]
pub enum MapError {
    /// The address requested to be mapped was out of the range supported by the page table
    /// configuration.
    #[error("Virtual address out of range")]
    AddressRange(VirtAddr),
    /// The end of the memory region is before the start.
    #[error("End of memory region {0:?} is before start.")]
    RegionBackwards(PageRangeInclusive),
    /// The requested flags are not supported for this mapping
    #[error("Flags {0:?} unsupported for mapping.")]
    InvalidFlags(PageTableFlags),
    /// The page range is already mapped.
    #[error("Atleast one page in the region {0:?} is already mapped.")]
    RegionAlreadyMapped(PageRangeInclusive),
    /// The page range is not mapped.
    #[error("Atleast one page in the region {0:?} is not mapped.")]
    RegionNotMapped(PageRangeInclusive),
    /// There was an error while updating a page table entry.
    #[error("Error updating page table entry {0:?}")]
    PteUpdateFault(PageTableEntry),
}

impl From<MapError> for crate::mmu::MmuError {
    fn from(e: MapError) -> Self {
        match e {
            MapError::AddressRange(a) => Self::BadAddress(a.as_u64() as usize),
            MapError::RegionBackwards(r) => Self::BadRegion(
                r.start.start_address().as_u64() as usize..r.end.start_address().as_u64() as usize,
            ),
            MapError::InvalidFlags(f) => Self::BadFlags(f.bits() as usize),
            MapError::RegionAlreadyMapped(r) => Self::BadRegion(
                r.start.start_address().as_u64() as usize..r.end.start_address().as_u64() as usize,
            ),
            MapError::RegionNotMapped(r) => Self::BadRegion(
                r.start.start_address().as_u64() as usize..r.end.start_address().as_u64() as usize,
            ),
            MapError::PteUpdateFault(pte) => Self::UpdateFailed {
                addr: pte.addr().as_u64() as usize,
                flags: Some(pte.flags().bits() as usize),
            },
        }
    }
}

// Identity mapping translation from Virtual to Physical address
fn from_virt_addr(vaddr: VirtAddr) -> PhysAddr {
    PhysAddr::new(vaddr.as_u64())
}

// Identity mapping translation from Page virtual to Physical address
fn from_page(page: Page<Size4KiB>) -> PhysFrame {
    PhysFrame::from_start_address(from_virt_addr(page.start_address())).unwrap()
}

type Cr3Val = (PhysFrame, Cr3Flags);

#[derive(Debug)]
struct IdMapPageTable {
    level: PageTableLevel,
    pagetable: Box<PageTable>,
    ptes: BTreeMap<PageTableIndex, IdMapPageTable>,
}

impl IdMapPageTable {
    const ROOT_LEVEL: PageTableLevel = PageTableLevel::Four;
    const PT_FLAGS: PageTableFlags = PageTableFlags::PRESENT.union(PageTableFlags::WRITABLE);

    fn new(level: PageTableLevel) -> Self {
        Self { level, pagetable: Box::new(PageTable::new()), ptes: BTreeMap::new() }
    }

    fn is_leaf(&self) -> bool {
        self.level == PageTableLevel::One
    }

    fn physaddr(&self) -> PhysAddr {
        PhysAddr::new(&*self.pagetable as *const _ as u64)
    }

    // Walk the page table for an already mapped page and invoke the callback
    // on the leaf page table entry.
    // Returns RegionNotMapped if called on an unmapped page.
    fn walk_mapped_page<F>(&mut self, page: &Page, f: F) -> Result<(), MapError>
    where
        F: Fn(&mut PageTableEntry) -> Result<(), MapError>,
    {
        if !self.is_leaf() {
            let index = page.page_table_index(self.level);
            let next_level_pt = self.ptes.get_mut(&index).ok_or_else(|| {
                let range = Page::range_inclusive(*page, *page);
                MapError::RegionNotMapped(range)
            })?;

            return next_level_pt.walk_mapped_page(page, f);
        }

        f(&mut self.pagetable[page.p1_index()])
    }

    fn identity_map(
        &mut self,
        page: Page<Size4KiB>,
        flags: PageTableFlags,
        flush: bool,
    ) -> Result<(), MapError> {
        if !self.is_leaf() {
            let index = page.page_table_index(self.level);
            let next_level_pt = self.ptes.entry(index).or_insert_with_key(|index| {
                let pte = &mut self.pagetable[*index];
                assert!(pte.is_unused());

                let pagetable = IdMapPageTable::new(self.level.next_lower_level().unwrap());
                pte.set_addr(pagetable.physaddr(), IdMapPageTable::PT_FLAGS);

                pagetable
            });
            return next_level_pt.identity_map(page, flags, flush);
        }

        if !self.pagetable[page.p1_index()].is_unused() {
            return Err(MapError::RegionAlreadyMapped(PageRangeInclusive {
                start: page,
                end: page,
            }));
        }
        self.pagetable[page.p1_index()].set_frame(from_page(page), flags);

        if flush {
            tlb_flush(page.start_address());
        }

        Ok(())
    }
}

/// Identity Mapped Page Table for x86_64.
/// This is a 4-level page table implementation and supports
/// only 4KiB Page size.
#[derive(Debug)]
pub struct IdMap {
    l4_pt: IdMapPageTable,
    old_cr3: Option<Cr3Val>,
}

impl IdMap {
    /// Creates a new instance with an empty level 4 pagetable.
    pub fn new() -> Self {
        let l4_pt = IdMapPageTable::new(IdMapPageTable::ROOT_LEVEL);
        Self { l4_pt, old_cr3: None }
    }

    fn l4_frame(&self) -> PhysFrame {
        PhysFrame::from_start_address(self.l4_pt.physaddr()).unwrap()
    }

    /// Activates the page table by programming CR3 register to point to the new
    /// level 4 pagetable. previous value of CR3 is stored so that it could be restored
    /// when this pagetable is deactivated(['deactivate`](Self::deactivate)).
    ///
    /// # SAFETY
    /// Caller should guarantee switching to a different translation is safe.
    pub unsafe fn activate(&mut self) {
        assert!(!self.is_active());

        self.old_cr3 = Some(Cr3::read());

        // SAFETY: level4 pagetable is validated in new().
        unsafe {
            Cr3::write(self.l4_frame(), Cr3Flags::empty());
        }
    }

    /// Deactivate the pagetable by programming CR3 register to the old value that was
    /// stored during [`activate`](Self::activate) call.
    /// # SAFETY: the caller should guaratnee switching to previous translation is safe.
    pub unsafe fn deactivate(&mut self) {
        assert!(self.is_active());

        let cr3_val = self.old_cr3.unwrap();
        self.old_cr3 = None;
        // SAFETY: Old value(Static PT) stored during activate
        unsafe {
            Cr3::write(cr3_val.0, cr3_val.1);
        }
    }

    fn is_active(&self) -> bool {
        self.old_cr3.is_some()
    }

    fn page_mapped(&mut self, page: &Page) -> bool {
        self.l4_pt
            .walk_mapped_page(page, |pte: &mut PageTableEntry| {
                if pte.is_unused() {
                    let range = Page::range_inclusive(*page, *page);
                    Err(MapError::RegionNotMapped(range))
                } else {
                    Ok(())
                }
            })
            .is_ok()
    }

    // Returns true if at least one page in the range is mapped.
    fn range_any_mapped(&mut self, region: &mut PageRangeInclusive) -> bool {
        for page in region {
            if self.page_mapped(&page) {
                return true;
            }
        }

        false
    }

    // Returns true if at least one page in the range is unmapped.
    fn range_any_unmapped(&mut self, region: &mut PageRangeInclusive) -> bool {
        for page in region {
            if !self.page_mapped(&page) {
                return true;
            }
        }

        false
    }

    fn validate_range(range: &PageRangeInclusive) -> Result<(), MapError> {
        if range.start > range.end {
            return Err(MapError::RegionBackwards(*range));
        }

        let last_addr = range.end.start_address().as_u64() + range.end.size() - 1;
        if last_addr >= IdMapPageTable::ROOT_LEVEL.table_address_space_alignment() {
            return Err(MapError::AddressRange(range.end.start_address()));
        }

        Ok(())
    }

    /// Maps the given range of Pages to identical page frames with the given flags.
    ///
    /// # Errors
    /// Returns [`MapError::RegionBackwards`] if the range is backwards.
    ///
    /// Returns [`MapError::AddressRange`] if the largest address in the `range` is greater than the
    /// largest virtual address covered by the page table(4 level table)
    ///
    /// Returns [`MapError::RegionAlreadyMapped`] if at least one of the pages in the range is
    /// already mapped and the page table is activated.
    ///
    /// Returns [`MapError::InvalidFlags`] if the `flags` argument has unsupported attributes set.
    pub fn map_range(
        &mut self,
        range: &mut PageRangeInclusive,
        flags: PageTableFlags,
    ) -> Result<(), MapError> {
        Self::validate_range(range)?;

        if flags.contains(INVALID_FLAGS) {
            return Err(MapError::InvalidFlags(flags));
        }

        // First pass: Verify that the range is not (partially) mapped.
        if self.is_active() && self.range_any_mapped(&mut range.clone()) {
            return Err(MapError::RegionAlreadyMapped(*range));
        }

        // Second pass: Perform the mapping.
        for page in range {
            self.l4_pt.identity_map(page, flags, self.is_active())?;
        }

        Ok(())
    }

    /// Unmaps the given range of pages
    ///
    /// # Errors
    /// Returns [`MapError::RegionBackwards`] if the range is backwards.
    ///
    /// Returns [`MapError::AddressRange`] if the largest address in the `range` is greater than the
    /// largest virtual address covered by the page table(4 level table)
    pub fn unmap_range(&mut self, range: &mut PageRangeInclusive) -> Result<(), MapError> {
        Self::validate_range(range)?;

        for page in range {
            self.l4_pt.walk_mapped_page(&page, |pte: &mut PageTableEntry| {
                pte.set_unused();
                Ok(())
            })?;
            tlb_flush(page.start_address());
        }

        Ok(())
    }

    /// Modify the flags of an already mapped range of pages
    ///
    /// # Errors
    /// Returns [`MapError::RegionBackwards`] if the range is backwards.
    ///
    /// Returns [`MapError::AddressRange`] if the largest address in the `range` is greater than the
    /// largest virtual address covered by the page table(4 level table)
    ///
    /// Returns [`MapError::RegionNotMapped`] if at least one of the pages in the range is
    /// not mapped.
    pub fn modify_range<F>(&mut self, range: &mut PageRangeInclusive, f: F) -> Result<(), MapError>
    where
        F: Fn(&mut PageTableEntry) -> Result<(), MapError>,
    {
        Self::validate_range(range)?;

        if self.range_any_unmapped(range) {
            return Err(MapError::RegionNotMapped(*range));
        }

        for page in range {
            self.l4_pt.walk_mapped_page(&page, &f)?;
            tlb_flush(page.start_address());
        }

        Ok(())
    }
}

impl Drop for IdMap {
    fn drop(&mut self) {
        // drop before deactivate is UB
        assert!(!self.is_active());
    }
}
