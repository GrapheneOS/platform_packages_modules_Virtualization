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
use crate::fdt::{read_initrd_range_from, read_kernel_range_from};
use core::num::NonZeroUsize;
use core::slice;
use log::debug;
use log::error;
use log::info;
use log::warn;
use vmbase::{
    layout::crosvm,
    memory::{map_data, map_rodata, resize_available_memory},
};

pub(crate) struct MemorySlices<'a> {
    pub fdt: &'a mut libfdt::Fdt,
    pub kernel: &'a [u8],
    pub ramdisk: Option<&'a [u8]>,
}

impl<'a> MemorySlices<'a> {
    pub fn new(fdt: usize, kernel: usize, kernel_size: usize) -> Result<Self, RebootReason> {
        let fdt_size = NonZeroUsize::new(crosvm::FDT_MAX_SIZE).unwrap();
        // TODO - Only map the FDT as read-only, until we modify it right before jump_to_payload()
        // e.g. by generating a DTBO for a template DT in main() and, on return, re-map DT as RW,
        // overwrite with the template DT and apply the DTBO.
        map_data(fdt, fdt_size).map_err(|e| {
            error!("Failed to allocate the FDT range: {e}");
            RebootReason::InternalError
        })?;

        // SAFETY: map_data validated the range to be in main memory, mapped, and not overlap.
        let untrusted_fdt = unsafe { slice::from_raw_parts_mut(fdt as *mut u8, fdt_size.into()) };
        let untrusted_fdt = libfdt::Fdt::from_mut_slice(untrusted_fdt).map_err(|e| {
            error!("Failed to load input FDT: {e}");
            RebootReason::InvalidFdt
        })?;

        let memory_range = untrusted_fdt.first_memory_range().map_err(|e| {
            error!("Failed to read memory range from DT: {e}");
            RebootReason::InvalidFdt
        })?;
        debug!("Resizing MemoryTracker to range {memory_range:#x?}");
        resize_available_memory(&memory_range).map_err(|e| {
            error!("Failed to use memory range value from DT: {memory_range:#x?}: {e}");
            RebootReason::InvalidFdt
        })?;

        let kernel_range = read_kernel_range_from(untrusted_fdt).map_err(|e| {
            error!("Failed to read kernel range: {e}");
            RebootReason::InvalidFdt
        })?;
        let (kernel_start, kernel_size) = if let Some(r) = kernel_range {
            (r.start, r.len())
        } else if cfg!(feature = "legacy") {
            warn!("Failed to find the kernel range in the DT; falling back to legacy ABI");
            (kernel, kernel_size)
        } else {
            error!("Failed to locate the kernel from the DT");
            return Err(RebootReason::InvalidPayload);
        };
        let kernel_size = kernel_size.try_into().map_err(|_| {
            error!("Invalid kernel size: {kernel_size:#x}");
            RebootReason::InvalidPayload
        })?;

        map_rodata(kernel_start, kernel_size).map_err(|e| {
            error!("Failed to map kernel range: {e}");
            RebootReason::InternalError
        })?;

        let kernel = kernel_start as *const u8;
        // SAFETY: map_rodata validated the range to be in main memory, mapped, and not overlap.
        let kernel = unsafe { slice::from_raw_parts(kernel, kernel_size.into()) };

        let initrd_range = read_initrd_range_from(untrusted_fdt).map_err(|e| {
            error!("Failed to read initrd range: {e}");
            RebootReason::InvalidFdt
        })?;
        let ramdisk = if let Some(r) = initrd_range {
            debug!("Located ramdisk at {r:?}");
            let ramdisk_size = r.len().try_into().map_err(|_| {
                error!("Invalid ramdisk size: {:#x}", r.len());
                RebootReason::InvalidRamdisk
            })?;
            map_rodata(r.start, ramdisk_size).map_err(|e| {
                error!("Failed to obtain the initrd range: {e}");
                RebootReason::InvalidRamdisk
            })?;

            // SAFETY: map_rodata validated the range to be in main memory, mapped, and not
            // overlap.
            Some(unsafe { slice::from_raw_parts(r.start as *const u8, r.len()) })
        } else {
            info!("Couldn't locate the ramdisk from the device tree");
            None
        };

        Ok(Self { fdt: untrusted_fdt, kernel, ramdisk })
    }
}
