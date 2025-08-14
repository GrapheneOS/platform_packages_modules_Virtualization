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
use core::slice;
use log::debug;
use log::error;
use log::info;
use log::warn;
use vmbase::{
    bzimage,
    layout::crosvm,
    memory::{map_data, map_rodata, resize_available_memory, MemoryTrackerError},
};
#[cfg(target_arch = "x86_64")]
use zerocopy::FromBytes;

pub(crate) struct MemorySlices<'a> {
    pub fdt: &'a mut libfdt::Fdt,
    pub kernel: &'a [u8],
    pub ramdisk: Option<&'a [u8]>,
    pub preserved_memory: Option<&'a [u8]>,
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    pub boot_params: Option<&'a mut bzimage::boot_params>,
}

fn map_data_slice_mut<'a>(addr: usize, size: usize) -> Result<&'a mut [u8], MemoryTrackerError> {
    let nonzero_size = size.try_into().map_err(|_| {
        error!("Invalid size specified for the range: {size:#x}");
        MemoryTrackerError::SizeTooSmall
    })?;
    map_data(addr, nonzero_size)?;

    // SAFETY: map_data validated the range to be in main memory, mapped, and not overlap.
    let mut_slice = unsafe { slice::from_raw_parts_mut(addr as *mut u8, size) };

    Ok(mut_slice)
}

fn map_data_slice<'a>(addr: usize, size: usize) -> Result<&'a [u8], MemoryTrackerError> {
    let nonzero_size = size.try_into().map_err(|e| {
        error!("Invalid size specified for the range: {e}");
        MemoryTrackerError::SizeTooSmall
    })?;
    map_rodata(addr, nonzero_size)?;

    // SAFETY: map_rodata validated the range to be in main memory, mapped, and not overlap.
    let slice = unsafe { slice::from_raw_parts(addr as *const u8, size) };

    Ok(slice)
}

#[cfg(target_arch = "x86_64")]
fn from_boot_args<'a>(
    boot_args: &BootArgs,
) -> Result<(Option<&'a mut bzimage::boot_params>, &'a mut [u8]), RebootReason> {
    let Some(boot_params_addr) = boot_args.boot_params else {
        error!("Missing boot_params address in boot_args");
        return Err(RebootReason::InvalidPayload);
    };

    let boot_params_slice = map_data_slice_mut(boot_params_addr, size_of::<bzimage::boot_params>())
        .map_err(|e| {
            error!("Failed to map the boot_params range: {e}");
            RebootReason::InternalError
        })?;
    let boot_params_ref = bzimage::boot_params::mut_from_bytes(boot_params_slice).unwrap();

    let setup_data_addr = boot_params_ref.hdr.setup_data();
    if setup_data_addr != crosvm::SETUP_DATA_START {
        error!(
            "setup_data not as per crosvm memory layout, actual: {:#x}, expected: {:#x}",
            setup_data_addr,
            crosvm::SETUP_DATA_START
        );
        return Err(RebootReason::InvalidPayload);
    }

    // Map the whole SETUP_DATA space which also include the fdt.
    let setup_data_slice =
        map_data_slice_mut(setup_data_addr, crosvm::SETUP_DATA_SIZE).map_err(|e| {
            error!("Failed to map the setup_data range: {e}");
            RebootReason::InternalError
        })?;

    if let Some(setup_dtb) =
        bzimage::find_setup_data_entry(setup_data_slice, bzimage::setup_data_header::SETUP_DTB)
    {
        if setup_dtb.len() > crosvm::FDT_MAX_SIZE {
            error!(
                "SETUP_DTB size is larger than expected maximum size ({} > {})!",
                setup_dtb.len(),
                crosvm::FDT_MAX_SIZE
            );
            return Err(RebootReason::InvalidPayload);
        }

        if let Some(fdt) = boot_args.fdt {
            if fdt != setup_dtb.as_ptr() as usize {
                error!("fdt present in boot_args, but doesn't match SETUP_DTB!");
                return Err(RebootReason::InternalError);
            }
        }
        return Ok((Some(boot_params_ref), setup_dtb));
    }

    error!("SETUP_DTB not found in setup_data!");
    Err(RebootReason::InvalidPayload)
}

#[cfg(target_arch = "aarch64")]
fn from_boot_args<'a>(
    boot_args: &BootArgs,
) -> Result<(Option<&'a mut bzimage::boot_params>, &'a mut [u8]), RebootReason> {
    let fdt: usize = boot_args.fdt.expect("Missing DT address");

    // TODO - Only map the FDT as read-only, until we modify it right before jump_to_payload()
    // e.g. by generating a DTBO for a template DT in main() and, on return, re-map DT as RW,
    // overwrite with the template DT and apply the DTBO.
    let fdt = map_data_slice_mut(fdt, crosvm::FDT_MAX_SIZE).map_err(|e| {
        error!("Failed to map the FDT range: {e}");
        RebootReason::InternalError
    })?;

    if boot_args.boot_params.is_some() {
        error!("boot_params is not expected to be present for arm64!");
        return Err(RebootReason::InternalError);
    }
    Ok((None, fdt))
}

impl<'a> MemorySlices<'a> {
    pub fn new(boot_args: BootArgs) -> Result<Self, RebootReason> {
        let (boot_params, fdt) = from_boot_args(&boot_args)?;

        let untrusted_fdt = libfdt::Fdt::from_mut_slice(fdt).map_err(|e| {
            error!("Failed to load input FDT: {e}");
            RebootReason::InvalidFdt
        })?;

        #[allow(unused_mut)]
        let mut memory_range = untrusted_fdt.first_memory_range().map_err(|e| {
            error!("Failed to read memory range from DT: {e}");
            RebootReason::InvalidFdt
        })?;
        // "/memory" may include the pvmfw region, which shouldn't be managed by MemoryTracker, so
        // truncate it off if present.
        #[cfg(target_arch = "aarch64")]
        if memory_range.start == crosvm::PVMFW_START {
            memory_range.start = crosvm::MEM_START;
        }
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
        } else if cfg!(feature = "compat-kernel-range-from-gprs") {
            warn!("Failed to find the kernel range in the DT; falling back to legacy ABI");
            (
                boot_args.payload_start.expect("Missing payload start in boot args"),
                boot_args.payload_size.expect("Missing payload size in boot args"),
            )
        } else {
            error!("Failed to locate the kernel from the DT");
            return Err(RebootReason::InvalidPayload);
        };

        let kernel = map_data_slice(kernel_start, kernel_size).map_err(|e| {
            error!("Failed to map kernel range: {e}");
            RebootReason::InternalError
        })?;

        // TODO(b/438297984): validate the initrd details match setup_header
        let initrd_range = read_initrd_range_from(untrusted_fdt).map_err(|e| {
            error!("Failed to read initrd range: {e}");
            RebootReason::InvalidFdt
        })?;
        let ramdisk = if let Some(r) = initrd_range {
            debug!("Located ramdisk at {r:?}");

            let ramdisk_slice = map_data_slice(r.start, r.len()).map_err(|e| {
                error!("Failed to obtain the initrd range: {e}");
                RebootReason::InvalidRamdisk
            })?;
            Some(ramdisk_slice)
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
