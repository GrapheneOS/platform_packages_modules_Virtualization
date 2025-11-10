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

//! acpi::Handler implementation

use crate::memory::{map_rodata, phys_to_virt, unmap, PAGE_SIZE};
use crate::util::unchecked_align_down;
use acpi::{aml::AmlError, Handle, Handler, PciAddress, PhysicalMapping};
use alloc::collections::{btree_map::Entry, BTreeMap};
use core::num::NonZeroUsize;
use log::warn;
use spin::mutex::SpinMutex;

// [un]map_physical_region can be of sub-page size and map/unmap to different regions
// with in a page may endup in MemoryTracker map/unmap calls to the same page.
// Since MemoryTracker works on page size granularity errors out if map is called on
// already mapped range, we need to handle it here before calling MemoryTracker.
//
// PER_PAGE_MAP_COUNTERS keep track of the regions that are mapped and prevents calling
// MemoryTracker's map if the region is already mapped and MemoryTracker's unmap
// is called only on the last unmap call of the same region from the library.
// MAPPINGS maintain a mapping from page aligned address of a frame and count of
// map requests. MemoryTracker's unmap is called when the count reaches 0.
static PAGE_REFCOUNTS: SpinMutex<BTreeMap<usize, usize>> = SpinMutex::new(BTreeMap::new());

#[derive(Clone)]
pub(super) struct AcpiHandler;

impl Handler for AcpiHandler {
    // # SAFETY
    // Caller should guarantee that the physical_address points to a
    // valid `T` in physical memory and size must be at least `sizeof::<T>()`
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<Self, T> {
        let mut page_refcounts = PAGE_REFCOUNTS.lock();

        let range = physical_address..physical_address + size;
        let mut mapped_length: usize = 0;
        range.step_by(PAGE_SIZE).map(|addr| unchecked_align_down(addr, PAGE_SIZE)).for_each(
            |addr| {
                page_refcounts.entry(addr).and_modify(|c| *c += 1).or_insert_with(|| {
                    map_rodata(addr, NonZeroUsize::new(PAGE_SIZE).unwrap()).unwrap();
                    1
                });
                mapped_length += PAGE_SIZE;
            },
        );

        PhysicalMapping {
            physical_start: physical_address,
            virtual_start: phys_to_virt(physical_address),
            region_length: size,
            mapped_length,
            handler: Self,
        }
    }

    fn unmap_physical_region<T>(region: &PhysicalMapping<Self, T>) {
        let mut page_refcounts = PAGE_REFCOUNTS.lock();

        let range = region.physical_start..region.physical_start + region.region_length;
        range.step_by(PAGE_SIZE).map(|addr| unchecked_align_down(addr, PAGE_SIZE)).for_each(
            |addr| match page_refcounts.entry(addr).and_modify(|c| *c -= 1) {
                Entry::Occupied(e) => {
                    if *e.get() == 0 {
                        unmap(addr, NonZeroUsize::new(PAGE_SIZE).unwrap()).unwrap();
                        e.remove();
                    }
                }
                Entry::Vacant(_) => {
                    warn!("Request to unmap address that is not mapped: {:#x?}", addr);
                }
            },
        );
    }

    // Our existing usecase (ECAM range retrieval) doesn't use any of the
    // following trait functions. Revisit this latter when we need them.

    fn read_u8(&self, _: usize) -> u8 {
        unimplemented!("read_u8")
    }
    fn read_u16(&self, _: usize) -> u16 {
        unimplemented!("read_u16")
    }
    fn read_u32(&self, _: usize) -> u32 {
        unimplemented!("read_u32")
    }
    fn read_u64(&self, _: usize) -> u64 {
        unimplemented!("read_u64")
    }
    fn write_u8(&self, _: usize, _: u8) {
        unimplemented!("write_u8")
    }
    fn write_u16(&self, _: usize, _: u16) {
        unimplemented!("write_u16")
    }
    fn write_u32(&self, _: usize, _: u32) {
        unimplemented!("write_u32")
    }
    fn write_u64(&self, _: usize, _: u64) {
        unimplemented!("write_u64")
    }
    fn read_io_u8(&self, _: u16) -> u8 {
        unimplemented!("read_io_u8")
    }
    fn read_io_u16(&self, _: u16) -> u16 {
        unimplemented!("read_io_u16")
    }
    fn read_io_u32(&self, _: u16) -> u32 {
        unimplemented!("read_io_u32")
    }
    fn write_io_u8(&self, _: u16, _: u8) {
        unimplemented!("write_io_u8")
    }
    fn write_io_u16(&self, _: u16, _: u16) {
        unimplemented!("write_io_u16")
    }
    fn write_io_u32(&self, _: u16, _: u32) {
        unimplemented!("write_io_u32")
    }
    fn read_pci_u8(&self, _: PciAddress, _: u16) -> u8 {
        unimplemented!("read_pci_u8")
    }
    fn read_pci_u16(&self, _: PciAddress, _: u16) -> u16 {
        unimplemented!("read_pci_u16")
    }
    fn read_pci_u32(&self, _: PciAddress, _: u16) -> u32 {
        unimplemented!("read_pci_u32")
    }
    fn write_pci_u8(&self, _: PciAddress, _: u16, _: u8) {
        unimplemented!("write_pci_u8")
    }
    fn write_pci_u16(&self, _: PciAddress, _: u16, _: u16) {
        unimplemented!("write_pci_u16")
    }
    fn write_pci_u32(&self, _: PciAddress, _: u16, _: u32) {
        unimplemented!("write_pci_u32")
    }
    fn nanos_since_boot(&self) -> u64 {
        unimplemented!("nanos_since_boot")
    }
    fn stall(&self, _: u64) {
        unimplemented!("stall")
    }
    fn sleep(&self, _: u64) {
        unimplemented!("sleep")
    }
    fn create_mutex(&self) -> Handle {
        unimplemented!("create_mutex")
    }
    fn acquire(&self, _: Handle, _: u16) -> Result<(), AmlError> {
        unimplemented!("acquire")
    }
    fn release(&self, _: Handle) {
        unimplemented!("release")
    }
}
