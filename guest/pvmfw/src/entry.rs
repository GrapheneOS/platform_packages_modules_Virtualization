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

//! Low-level entry and exit points of pvmfw.

use crate::config;
use crate::fdt;
use crate::memory;
use core::arch::asm;
use core::mem::{drop, size_of};
use core::num::NonZeroUsize;
use core::ops::Range;
use core::slice;
use log::debug;
use log::error;
use log::info;
use log::warn;
use log::LevelFilter;
use vmbase::util::RangeExt as _;
use vmbase::{
    configure_heap, console_writeln,
    hyp::{get_mem_sharer, get_mmio_guard},
    layout::{self, crosvm, UART_PAGE_ADDR},
    main,
    memory::{min_dcache_line_size, MemoryTracker, MEMORY, SIZE_128KB, SIZE_4KB},
    power::reboot,
};
use zeroize::Zeroize;

#[derive(Debug, Clone)]
pub enum RebootReason {
    /// A malformed BCC was received.
    InvalidBcc,
    /// An invalid configuration was appended to pvmfw.
    InvalidConfig,
    /// An unexpected internal error happened.
    InternalError,
    /// The provided FDT was invalid.
    InvalidFdt,
    /// The provided payload was invalid.
    InvalidPayload,
    /// The provided ramdisk was invalid.
    InvalidRamdisk,
    /// Failed to verify the payload.
    PayloadVerificationError,
    /// DICE layering process failed.
    SecretDerivationError,
}

impl RebootReason {
    pub fn as_avf_reboot_string(&self) -> &'static str {
        match self {
            Self::InvalidBcc => "PVM_FIRMWARE_INVALID_BCC",
            Self::InvalidConfig => "PVM_FIRMWARE_INVALID_CONFIG_DATA",
            Self::InternalError => "PVM_FIRMWARE_INTERNAL_ERROR",
            Self::InvalidFdt => "PVM_FIRMWARE_INVALID_FDT",
            Self::InvalidPayload => "PVM_FIRMWARE_INVALID_PAYLOAD",
            Self::InvalidRamdisk => "PVM_FIRMWARE_INVALID_RAMDISK",
            Self::PayloadVerificationError => "PVM_FIRMWARE_PAYLOAD_VERIFICATION_FAILED",
            Self::SecretDerivationError => "PVM_FIRMWARE_SECRET_DERIVATION_FAILED",
        }
    }
}

main!(start);
configure_heap!(SIZE_128KB);

/// Entry point for pVM firmware.
pub fn start(fdt_address: u64, payload_start: u64, payload_size: u64, _arg3: u64) {
    // Limitations in this function:
    // - can't access non-pvmfw memory (only statically-mapped memory)
    // - can't access MMIO (except the console, already configured by vmbase)

    match main_wrapper(fdt_address as usize, payload_start as usize, payload_size as usize) {
        Ok((entry, bcc)) => jump_to_payload(fdt_address, entry.try_into().unwrap(), bcc),
        Err(e) => {
            const REBOOT_REASON_CONSOLE: usize = 1;
            console_writeln!(REBOOT_REASON_CONSOLE, "{}", e.as_avf_reboot_string());
            reboot()
        }
    }

    // if we reach this point and return, vmbase::entry::rust_entry() will call power::shutdown().
}

struct MemorySlices<'a> {
    fdt: &'a mut libfdt::Fdt,
    kernel: &'a [u8],
    ramdisk: Option<&'a [u8]>,
}

impl<'a> MemorySlices<'a> {
    fn new(
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

/// Sets up the environment for main() and wraps its result for start().
///
/// Provide the abstractions necessary for start() to abort the pVM boot and for main() to run with
/// the assumption that its environment has been properly configured.
fn main_wrapper(
    fdt: usize,
    payload: usize,
    payload_size: usize,
) -> Result<(usize, Range<usize>), RebootReason> {
    // Limitations in this function:
    // - only access MMIO once (and while) it has been mapped and configured
    // - only perform logging once the logger has been initialized
    // - only access non-pvmfw memory once (and while) it has been mapped

    log::set_max_level(LevelFilter::Info);

    let page_table = memory::init_page_table().map_err(|e| {
        error!("Failed to set up the dynamic page tables: {e}");
        RebootReason::InternalError
    })?;

    // SAFETY: We only get the appended payload from here, once. The region was statically mapped,
    // then remapped by `init_page_table()`.
    let appended_data = unsafe { get_appended_data_slice() };

    let appended = AppendedPayload::new(appended_data).ok_or_else(|| {
        error!("No valid configuration found");
        RebootReason::InvalidConfig
    })?;

    let config_entries = appended.get_entries();

    // Up to this point, we were using the built-in static (from .rodata) page tables.
    MEMORY.lock().replace(MemoryTracker::new(
        page_table,
        crosvm::MEM_START..layout::MAX_VIRT_ADDR,
        crosvm::MMIO_RANGE,
        Some(memory::appended_payload_range()),
    ));

    let slices = MemorySlices::new(
        fdt,
        payload,
        payload_size,
        config_entries.vm_dtbo,
        config_entries.vm_ref_dt,
    )?;

    // This wrapper allows main() to be blissfully ignorant of platform details.
    let (next_bcc, debuggable_payload) = crate::main(
        slices.fdt,
        slices.kernel,
        slices.ramdisk,
        config_entries.bcc,
        config_entries.debug_policy,
    )?;

    // Writable-dirty regions will be flushed when MemoryTracker is dropped.
    config_entries.bcc.zeroize();

    info!("Expecting a bug making MMIO_GUARD_UNMAP return NOT_SUPPORTED on success");
    MEMORY.lock().as_mut().unwrap().unshare_all_mmio().map_err(|e| {
        error!("Failed to unshare MMIO ranges: {e}");
        RebootReason::InternalError
    })?;
    // Call unshare_all_memory here (instead of relying on the dtor) while UART is still mapped.
    MEMORY.lock().as_mut().unwrap().unshare_all_memory();

    if let Some(mmio_guard) = get_mmio_guard() {
        if cfg!(debuggable_vms_improvements) && debuggable_payload {
            // Keep UART MMIO_GUARD-ed for debuggable payloads, to enable earlycon.
        } else {
            mmio_guard.unmap(UART_PAGE_ADDR).map_err(|e| {
                error!("Failed to unshare the UART: {e}");
                RebootReason::InternalError
            })?;
        }
    }

    // Drop MemoryTracker and deactivate page table.
    drop(MEMORY.lock().take());

    Ok((slices.kernel.as_ptr() as usize, next_bcc))
}

fn jump_to_payload(fdt_address: u64, payload_start: u64, bcc: Range<usize>) -> ! {
    const ASM_STP_ALIGN: usize = size_of::<u64>() * 2;
    const SCTLR_EL1_RES1: u64 = (0b11 << 28) | (0b101 << 20) | (0b1 << 11);
    // Stage 1 instruction access cacheability is unaffected.
    const SCTLR_EL1_I: u64 = 0b1 << 12;
    // SETEND instruction disabled at EL0 in aarch32 mode.
    const SCTLR_EL1_SED: u64 = 0b1 << 8;
    // Various IT instructions are disabled at EL0 in aarch32 mode.
    const SCTLR_EL1_ITD: u64 = 0b1 << 7;

    const SCTLR_EL1_VAL: u64 = SCTLR_EL1_RES1 | SCTLR_EL1_ITD | SCTLR_EL1_SED | SCTLR_EL1_I;

    let scratch = layout::scratch_range();

    assert_ne!(scratch.end - scratch.start, 0, "scratch memory is empty.");
    assert_eq!(scratch.start.0 % ASM_STP_ALIGN, 0, "scratch memory is misaligned.");
    assert_eq!(scratch.end.0 % ASM_STP_ALIGN, 0, "scratch memory is misaligned.");

    assert!(bcc.is_within(&(scratch.start.0..scratch.end.0)));
    assert_eq!(bcc.start % ASM_STP_ALIGN, 0, "Misaligned guest BCC.");
    assert_eq!(bcc.end % ASM_STP_ALIGN, 0, "Misaligned guest BCC.");

    let stack = memory::stack_range();

    assert_ne!(stack.end - stack.start, 0, "stack region is empty.");
    assert_eq!(stack.start.0 % ASM_STP_ALIGN, 0, "Misaligned stack region.");
    assert_eq!(stack.end.0 % ASM_STP_ALIGN, 0, "Misaligned stack region.");

    // Zero all memory that could hold secrets and that can't be safely written to from Rust.
    // Disable the exception vector, caches and page table and then jump to the payload at the
    // given address, passing it the given FDT pointer.
    //
    // SAFETY: We're exiting pvmfw by passing the register values we need to a noreturn asm!().
    unsafe {
        asm!(
            "cmp {scratch}, {bcc}",
            "b.hs 1f",

            // Zero .data & .bss until BCC.
            "0: stp xzr, xzr, [{scratch}], 16",
            "cmp {scratch}, {bcc}",
            "b.lo 0b",

            "1:",
            // Skip BCC.
            "mov {scratch}, {bcc_end}",
            "cmp {scratch}, {scratch_end}",
            "b.hs 1f",

            // Keep zeroing .data & .bss.
            "0: stp xzr, xzr, [{scratch}], 16",
            "cmp {scratch}, {scratch_end}",
            "b.lo 0b",

            "1:",
            // Flush d-cache over .data & .bss (including BCC).
            "0: dc cvau, {cache_line}",
            "add {cache_line}, {cache_line}, {dcache_line_size}",
            "cmp {cache_line}, {scratch_end}",
            "b.lo 0b",

            "mov {cache_line}, {stack}",
            // Zero stack region.
            "0: stp xzr, xzr, [{stack}], 16",
            "cmp {stack}, {stack_end}",
            "b.lo 0b",

            // Flush d-cache over stack region.
            "0: dc cvau, {cache_line}",
            "add {cache_line}, {cache_line}, {dcache_line_size}",
            "cmp {cache_line}, {stack_end}",
            "b.lo 0b",

            "msr sctlr_el1, {sctlr_el1_val}",
            "isb",
            "mov x1, xzr",
            "mov x2, xzr",
            "mov x3, xzr",
            "mov x4, xzr",
            "mov x5, xzr",
            "mov x6, xzr",
            "mov x7, xzr",
            "mov x8, xzr",
            "mov x9, xzr",
            "mov x10, xzr",
            "mov x11, xzr",
            "mov x12, xzr",
            "mov x13, xzr",
            "mov x14, xzr",
            "mov x15, xzr",
            "mov x16, xzr",
            "mov x17, xzr",
            "mov x18, xzr",
            "mov x19, xzr",
            "mov x20, xzr",
            "mov x21, xzr",
            "mov x22, xzr",
            "mov x23, xzr",
            "mov x24, xzr",
            "mov x25, xzr",
            "mov x26, xzr",
            "mov x27, xzr",
            "mov x28, xzr",
            "mov x29, xzr",
            "msr ttbr0_el1, xzr",
            // Ensure that CMOs have completed before entering payload.
            "dsb nsh",
            "br x30",
            sctlr_el1_val = in(reg) SCTLR_EL1_VAL,
            bcc = in(reg) u64::try_from(bcc.start).unwrap(),
            bcc_end = in(reg) u64::try_from(bcc.end).unwrap(),
            cache_line = in(reg) u64::try_from(scratch.start.0).unwrap(),
            scratch = in(reg) u64::try_from(scratch.start.0).unwrap(),
            scratch_end = in(reg) u64::try_from(scratch.end.0).unwrap(),
            stack = in(reg) u64::try_from(stack.start.0).unwrap(),
            stack_end = in(reg) u64::try_from(stack.end.0).unwrap(),
            dcache_line_size = in(reg) u64::try_from(min_dcache_line_size()).unwrap(),
            in("x0") fdt_address,
            in("x30") payload_start,
            options(noreturn),
        );
    };
}

/// # Safety
///
/// This must only be called once, since we are returning a mutable reference.
/// The appended data region must be mapped.
unsafe fn get_appended_data_slice() -> &'static mut [u8] {
    let range = memory::appended_payload_range();
    // SAFETY: This region is mapped and the linker script prevents it from overlapping with other
    // objects.
    unsafe { slice::from_raw_parts_mut(range.start.0 as *mut u8, range.end - range.start) }
}

enum AppendedPayload<'a> {
    /// Configuration data.
    Config(config::Config<'a>),
    /// Deprecated raw BCC, as used in Android T.
    LegacyBcc(&'a mut [u8]),
}

impl<'a> AppendedPayload<'a> {
    fn new(data: &'a mut [u8]) -> Option<Self> {
        // The borrow checker gets confused about the ownership of data (see inline comments) so we
        // intentionally obfuscate it using a raw pointer; see a similar issue (still not addressed
        // in v1.77) in https://users.rust-lang.org/t/78467.
        let data_ptr = data as *mut [u8];

        // Config::new() borrows data as mutable ...
        match config::Config::new(data) {
            // ... so this branch has a mutable reference to data, from the Ok(Config<'a>). But ...
            Ok(valid) => Some(Self::Config(valid)),
            // ... if Config::new(data).is_err(), the Err holds no ref to data. However ...
            Err(config::Error::InvalidMagic) if cfg!(feature = "legacy") => {
                // ... the borrow checker still complains about a second mutable ref without this.
                // SAFETY: Pointer to a valid mut (not accessed elsewhere), 'a lifetime re-used.
                let data: &'a mut _ = unsafe { &mut *data_ptr };

                const BCC_SIZE: usize = SIZE_4KB;
                warn!("Assuming the appended data at {:?} to be a raw BCC", data.as_ptr());
                Some(Self::LegacyBcc(&mut data[..BCC_SIZE]))
            }
            Err(e) => {
                error!("Invalid configuration data at {data_ptr:?}: {e}");
                None
            }
        }
    }

    fn get_entries(self) -> config::Entries<'a> {
        match self {
            Self::Config(cfg) => cfg.get_entries(),
            Self::LegacyBcc(bcc) => config::Entries { bcc, ..Default::default() },
        }
    }
}
