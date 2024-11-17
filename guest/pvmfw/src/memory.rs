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

//! Low-level allocation and tracking of main memory.

use crate::entry::RebootReason;
use crate::fdt;
use crate::helpers::PVMFW_PAGE_SIZE;
use aarch64_paging::paging::VirtualAddress;
use aarch64_paging::MapError;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::result;
use core::slice;
use hypervisor_backends::get_mem_sharer;
use log::debug;
use log::error;
use log::info;
use log::warn;
use vmbase::{
    layout::{self, crosvm},
    memory::{PageTable, MEMORY, SIZE_2MB, SIZE_4KB},
    util::align_up,
};

/// Returns memory range reserved for the appended payload.
pub fn appended_payload_range() -> Range<VirtualAddress> {
    let start = align_up(layout::binary_end().0, SIZE_4KB).unwrap();
    // pvmfw is contained in a 2MiB region so the payload can't be larger than the 2MiB alignment.
    let end = align_up(start, SIZE_2MB).unwrap();
    VirtualAddress(start)..VirtualAddress(end)
}

/// Region allocated for the stack.
pub fn stack_range() -> Range<VirtualAddress> {
    const STACK_PAGES: usize = 12;

    layout::stack_range(STACK_PAGES * PVMFW_PAGE_SIZE)
}

pub fn init_page_table() -> result::Result<PageTable, MapError> {
    let mut page_table = PageTable::default();

    // Stack and scratch ranges are explicitly zeroed and flushed before jumping to payload,
    // so dirty state management can be omitted.
    page_table.map_data(&layout::scratch_range().into())?;
    page_table.map_data(&stack_range().into())?;
    page_table.map_code(&layout::text_range().into())?;
    page_table.map_rodata(&layout::rodata_range().into())?;
    page_table.map_data_dbm(&appended_payload_range().into())?;
    if let Err(e) = page_table.map_device(&layout::console_uart_page().into()) {
        error!("Failed to remap the UART as a dynamic page table entry: {e}");
        return Err(e);
    }
    Ok(page_table)
}

pub(crate) struct MemorySlices<'a> {
    pub fdt: &'a mut libfdt::Fdt,
    pub kernel: &'a [u8],
    pub ramdisk: Option<&'a [u8]>,
}

impl<'a> MemorySlices<'a> {
    pub fn new(
        fdt: usize,
        kernel: usize,
        kernel_size: usize,
        vm_dtbo: Option<&mut [u8]>,
        vm_ref_dt: Option<&[u8]>,
    ) -> Result<Self, RebootReason> {
        let fdt_size = NonZeroUsize::new(crosvm::FDT_MAX_SIZE).unwrap();
        // TODO - Only map the FDT as read-only, until we modify it right before jump_to_payload()
        // e.g. by generating a DTBO for a template DT in main() and, on return, re-map DT as RW,
        // overwrite with the template DT and apply the DTBO.
        let range = MEMORY.lock().as_mut().unwrap().alloc_mut(fdt, fdt_size).map_err(|e| {
            error!("Failed to allocate the FDT range: {e}");
            RebootReason::InternalError
        })?;

        // SAFETY: The tracker validated the range to be in main memory, mapped, and not overlap.
        let fdt = unsafe { slice::from_raw_parts_mut(range.start as *mut u8, range.len()) };

        let info = fdt::sanitize_device_tree(fdt, vm_dtbo, vm_ref_dt)?;
        let fdt = libfdt::Fdt::from_mut_slice(fdt).map_err(|e| {
            error!("Failed to load sanitized FDT: {e}");
            RebootReason::InvalidFdt
        })?;
        debug!("Fdt passed validation!");

        let memory_range = info.memory_range;
        debug!("Resizing MemoryTracker to range {memory_range:#x?}");
        MEMORY.lock().as_mut().unwrap().shrink(&memory_range).map_err(|e| {
            error!("Failed to use memory range value from DT: {memory_range:#x?}: {e}");
            RebootReason::InvalidFdt
        })?;

        if let Some(mem_sharer) = get_mem_sharer() {
            let granule = mem_sharer.granule().map_err(|e| {
                error!("Failed to get memory protection granule: {e}");
                RebootReason::InternalError
            })?;
            MEMORY.lock().as_mut().unwrap().init_dynamic_shared_pool(granule).map_err(|e| {
                error!("Failed to initialize dynamically shared pool: {e}");
                RebootReason::InternalError
            })?;
        } else {
            let range = info.swiotlb_info.fixed_range().ok_or_else(|| {
                error!("Pre-shared pool range not specified in swiotlb node");
                RebootReason::InvalidFdt
            })?;

            MEMORY.lock().as_mut().unwrap().init_static_shared_pool(range).map_err(|e| {
                error!("Failed to initialize pre-shared pool {e}");
                RebootReason::InvalidFdt
            })?;
        }

        let kernel_range = if let Some(r) = info.kernel_range {
            MEMORY.lock().as_mut().unwrap().alloc_range(&r).map_err(|e| {
                error!("Failed to obtain the kernel range with DT range: {e}");
                RebootReason::InternalError
            })?
        } else if cfg!(feature = "legacy") {
            warn!("Failed to find the kernel range in the DT; falling back to legacy ABI");

            let kernel_size = NonZeroUsize::new(kernel_size).ok_or_else(|| {
                error!("Invalid kernel size: {kernel_size:#x}");
                RebootReason::InvalidPayload
            })?;

            MEMORY.lock().as_mut().unwrap().alloc(kernel, kernel_size).map_err(|e| {
                error!("Failed to obtain the kernel range with legacy range: {e}");
                RebootReason::InternalError
            })?
        } else {
            error!("Failed to locate the kernel from the DT");
            return Err(RebootReason::InvalidPayload);
        };

        let kernel = kernel_range.start as *const u8;
        // SAFETY: The tracker validated the range to be in main memory, mapped, and not overlap.
        let kernel = unsafe { slice::from_raw_parts(kernel, kernel_range.len()) };

        let ramdisk = if let Some(r) = info.initrd_range {
            debug!("Located ramdisk at {r:?}");
            let r = MEMORY.lock().as_mut().unwrap().alloc_range(&r).map_err(|e| {
                error!("Failed to obtain the initrd range: {e}");
                RebootReason::InvalidRamdisk
            })?;

            // SAFETY: The region was validated by memory to be in main memory, mapped, and
            // not overlap.
            Some(unsafe { slice::from_raw_parts(r.start as *const u8, r.len()) })
        } else {
            info!("Couldn't locate the ramdisk from the device tree");
            None
        };

        Ok(Self { fdt, kernel, ramdisk })
    }
}
