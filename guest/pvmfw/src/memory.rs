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

use crate::entry::{BootArgs, RebootReason};
use crate::fdt::{read_initrd_range_from, read_kernel_range_from};
use core::num::NonZeroUsize;
use core::slice;
use log::debug;
use log::error;
use log::info;
use log::warn;
use vmbase::{
    bzimage,
    layout::crosvm,
    memory::{map_data, map_rodata, resize_available_memory},
};
use zerocopy::FromBytes;

pub(crate) struct MemorySlices<'a> {
    pub fdt: &'a mut libfdt::Fdt,
    pub kernel: &'a [u8],
    pub ramdisk: Option<&'a [u8]>,
    pub preserved_memory: Option<&'a [u8]>,
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    pub boot_params: Option<&'a mut bzimage::boot_params>,
}

impl<'a> MemorySlices<'a> {
    pub fn new(boot_args: BootArgs) -> Result<Self, RebootReason> {
        let mut boot_params = None;

        if let Some(boot_params_addr) = boot_args.boot_params {
            let boot_params_size = NonZeroUsize::new(size_of::<bzimage::boot_params>()).unwrap();
            map_data(boot_params_addr, boot_params_size).map_err(|e| {
                error!("Failed to map the boot_params range: {e}");
                RebootReason::InternalError
            })?;

            // SAFETY: map_data validated the range to be in main memory, mapped, and not overlap.
            let boot_params_slice = unsafe {
                slice::from_raw_parts_mut(boot_params_addr as *mut u8, boot_params_size.into())
            };
            let boot_params_ref = bzimage::boot_params::mut_from_bytes(boot_params_slice).unwrap();
            boot_params = Some(boot_params_ref);
        }

        let fdt: usize = boot_args.fdt.expect("Missing DT address");
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
            (
                boot_args.payload_start.expect("Missing payload start in boot args"),
                boot_args.payload_size.expect("Missing payload size in boot args"),
            )
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

        let preserved_memory = None;

        Ok(Self { fdt: untrusted_fdt, kernel, ramdisk, preserved_memory, boot_params })
    }

    pub fn add_preserved_memory(&mut self, slice: &'a [u8]) {
        self.preserved_memory = Some(slice)
    }
}
