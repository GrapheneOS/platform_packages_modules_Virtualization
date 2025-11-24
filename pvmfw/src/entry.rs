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
use crate::crypto;
use crate::fdt;
use crate::heap;
use crate::helpers;
use crate::helpers::RangeExt as _;
use crate::memory::{MemoryTracker, MEMORY};
use crate::mmu;
use crate::rand;
use core::arch::asm;
use core::mem::size_of;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::slice;
use hyp::{get_hypervisor, HypervisorCap};
use log::debug;
use log::error;
use log::info;
use log::warn;
use log::LevelFilter;
use vmbase::{console, layout, logger, main, power::reboot};

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

main!(start);

/// Entry point for pVM firmware.
pub fn start(fdt_address: u64, payload_start: u64, payload_size: u64, _arg3: u64) {
    // Limitations in this function:
    // - can't access non-pvmfw memory (only statically-mapped memory)
    // - can't access MMIO (therefore, no logging)

    match main_wrapper(fdt_address as usize, payload_start as usize, payload_size as usize) {
        Ok((entry, bcc)) => jump_to_payload(fdt_address, entry.try_into().unwrap(), bcc),
        Err(_) => reboot(), // TODO(b/220071963) propagate the reason back to the host.
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
        memory: &mut MemoryTracker,
    ) -> Result<Self, RebootReason> {
        // SAFETY - SIZE_2MB is non-zero.
        const FDT_SIZE: NonZeroUsize = unsafe { NonZeroUsize::new_unchecked(helpers::SIZE_2MB) };
        // TODO - Only map the FDT as read-only, until we modify it right before jump_to_payload()
        // e.g. by generating a DTBO for a template DT in main() and, on return, re-map DT as RW,
        // overwrite with the template DT and apply the DTBO.
        let range = memory.alloc_mut(fdt, FDT_SIZE).map_err(|e| {
            error!("Failed to allocate the FDT range: {e}");
            RebootReason::InternalError
        })?;

        // SAFETY - The tracker validated the range to be in main memory, mapped, and not overlap.
        let fdt = unsafe { slice::from_raw_parts_mut(range.start as *mut u8, range.len()) };
        let fdt = libfdt::Fdt::from_mut_slice(fdt).map_err(|e| {
            error!("Failed to spawn the FDT wrapper: {e}");
            RebootReason::InvalidFdt
        })?;

        let info = fdt::sanitize_device_tree(fdt)?;
        debug!("Fdt passed validation!");

        let memory_range = info.memory_range;
        debug!("Resizing MemoryTracker to range {memory_range:#x?}");
        memory.shrink(&memory_range).map_err(|_| {
            error!("Failed to use memory range value from DT: {memory_range:#x?}");
            RebootReason::InvalidFdt
        })?;

        if get_hypervisor().has_cap(HypervisorCap::DYNAMIC_MEM_SHARE) {
            memory.init_dynamic_shared_pool().map_err(|e| {
                error!("Failed to initialize dynamically shared pool: {e}");
                RebootReason::InternalError
            })?;
        } else {
            let range = info.swiotlb_info.fixed_range().ok_or_else(|| {
                error!("Pre-shared pool range not specified in swiotlb node");
                RebootReason::InvalidFdt
            })?;

            memory.init_static_shared_pool(range).map_err(|e| {
                error!("Failed to initialize pre-shared pool {e}");
                RebootReason::InvalidFdt
            })?;
        }

        let kernel_range = if let Some(r) = info.kernel_range {
            memory.alloc_range(&r).map_err(|e| {
                error!("Failed to obtain the kernel range with DT range: {e}");
                RebootReason::InternalError
            })?
        } else if cfg!(feature = "legacy") {
            warn!("Failed to find the kernel range in the DT; falling back to legacy ABI");

            let kernel_size = NonZeroUsize::new(kernel_size).ok_or_else(|| {
                error!("Invalid kernel size: {kernel_size:#x}");
                RebootReason::InvalidPayload
            })?;

            memory.alloc(kernel, kernel_size).map_err(|e| {
                error!("Failed to obtain the kernel range with legacy range: {e}");
                RebootReason::InternalError
            })?
        } else {
            error!("Failed to locate the kernel from the DT");
            return Err(RebootReason::InvalidPayload);
        };

        // SAFETY - The tracker validated the range to be in main memory, mapped, and not overlap.
        let kernel =
            unsafe { slice::from_raw_parts(kernel_range.start as *const u8, kernel_range.len()) };

        let ramdisk = if let Some(r) = info.initrd_range {
            debug!("Located ramdisk at {r:?}");
            let r = memory.alloc_range(&r).map_err(|e| {
                error!("Failed to obtain the initrd range: {e}");
                RebootReason::InvalidRamdisk
            })?;

            // SAFETY - The region was validated by memory to be in main memory, mapped, and
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

    // SAFETY - This function should and will only be called once, here.
    unsafe { heap::init() };

    logger::init(LevelFilter::Info).map_err(|_| RebootReason::InternalError)?;

    // Use debug!() to avoid printing to the UART if we failed to configure it as only local
    // builds that have tweaked the logger::init() call will actually attempt to log the message.

    get_hypervisor().mmio_guard_init().map_err(|e| {
        debug!("{e}");
        RebootReason::InternalError
    })?;

    get_hypervisor().mmio_guard_map(console::BASE_ADDRESS).map_err(|e| {
        debug!("Failed to configure the UART: {e}");
        RebootReason::InternalError
    })?;

    crypto::init();

    // SAFETY - We only get the appended payload from here, once. It is mapped and the linker
    // script prevents it from overlapping with other objects.
    let appended_data = unsafe { get_appended_data_slice() };

    // Up to this point, we were using the built-in static (from .rodata) page tables.

    let mut page_table = mmu::PageTable::from_static_layout().map_err(|e| {
        error!("Failed to set up the dynamic page tables: {e}");
        RebootReason::InternalError
    })?;

    const CONSOLE_LEN: usize = 1; // vmbase::uart::Uart only uses one u8 register.
    let uart_range = console::BASE_ADDRESS..(console::BASE_ADDRESS + CONSOLE_LEN);
    page_table.map_device(&uart_range).map_err(|e| {
        error!("Failed to remap the UART as a dynamic page table entry: {e}");
        RebootReason::InternalError
    })?;

    // SAFETY - We only get the appended payload from here, once. It is statically mapped and the
    // linker script prevents it from overlapping with other objects.
    let mut appended = unsafe { AppendedPayload::new(appended_data) }.ok_or_else(|| {
        error!("No valid configuration found");
        RebootReason::InvalidConfig
    })?;

    let (bcc_slice, debug_policy) = appended.get_entries();

    debug!("Activating dynamic page table...");
    // SAFETY - page_table duplicates the static mappings for everything that the Rust code is
    // aware of so activating it shouldn't have any visible effect.
    unsafe { page_table.activate() };
    debug!("... Success!");

    MEMORY.lock().replace(MemoryTracker::new(page_table));
    let slices = MemorySlices::new(fdt, payload, payload_size, MEMORY.lock().as_mut().unwrap())?;

    rand::init().map_err(|e| {
        error!("Failed to initialize rand: {e}");
        RebootReason::InternalError
    })?;

    // This wrapper allows main() to be blissfully ignorant of platform details.
    let next_bcc = crate::main(
        slices.fdt,
        slices.kernel,
        slices.ramdisk,
        bcc_slice,
        debug_policy,
        MEMORY.lock().as_mut().unwrap(),
    )?;

    helpers::flushed_zeroize(bcc_slice);

    info!("Expecting a bug making MMIO_GUARD_UNMAP return NOT_SUPPORTED on success");
    MEMORY.lock().as_mut().unwrap().mmio_unmap_all().map_err(|e| {
        error!("Failed to unshare MMIO ranges: {e}");
        RebootReason::InternalError
    })?;
    // Call unshare_all_memory here (instead of relying on the dtor) while UART is still mapped.
    MEMORY.lock().as_mut().unwrap().unshare_all_memory();
    get_hypervisor().mmio_guard_unmap(console::BASE_ADDRESS).map_err(|e| {
        error!("Failed to unshare the UART: {e}");
        RebootReason::InternalError
    })?;
    MEMORY.lock().take().unwrap();

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

    assert_ne!(scratch.len(), 0, "scratch memory is empty.");
    assert_eq!(scratch.start % ASM_STP_ALIGN, 0, "scratch memory is misaligned.");
    assert_eq!(scratch.end % ASM_STP_ALIGN, 0, "scratch memory is misaligned.");

    assert!(bcc.is_within(&scratch));
    assert_eq!(bcc.start % ASM_STP_ALIGN, 0, "Misaligned guest BCC.");
    assert_eq!(bcc.end % ASM_STP_ALIGN, 0, "Misaligned guest BCC.");

    let stack = mmu::stack_range();

    assert_ne!(stack.len(), 0, "stack region is empty.");
    assert_eq!(stack.start % ASM_STP_ALIGN, 0, "Misaligned stack region.");
    assert_eq!(stack.end % ASM_STP_ALIGN, 0, "Misaligned stack region.");

    // Zero all memory that could hold secrets and that can't be safely written to from Rust.
    // Disable the exception vector, caches and page table and then jump to the payload at the
    // given address, passing it the given FDT pointer.
    //
    // SAFETY - We're exiting pvmfw by passing the register values we need to a noreturn asm!().
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
            "0: dc cvac, {cache_line}",
            "add {cache_line}, {cache_line}, {dcache_line_size}",
            "cmp {cache_line}, {scratch_end}",
            "b.lo 0b",

            "mov {cache_line}, {stack}",
            // Zero stack region.
            "0: stp xzr, xzr, [{stack}], 16",
            "cmp {stack}, {stack_end}",
            "b.lo 0b",

            // Flush d-cache over stack region.
            "0: dc cvac, {cache_line}",
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
            cache_line = in(reg) u64::try_from(scratch.start).unwrap(),
            scratch = in(reg) u64::try_from(scratch.start).unwrap(),
            scratch_end = in(reg) u64::try_from(scratch.end).unwrap(),
            stack = in(reg) u64::try_from(stack.start).unwrap(),
            stack_end = in(reg) u64::try_from(stack.end).unwrap(),
            dcache_line_size = in(reg) u64::try_from(helpers::min_dcache_line_size()).unwrap(),
            in("x0") fdt_address,
            in("x30") payload_start,
            options(noreturn),
        );
    };
}

unsafe fn get_appended_data_slice() -> &'static mut [u8] {
    let base = helpers::align_up(layout::binary_end(), helpers::SIZE_4KB).unwrap();
    // pvmfw is contained in a 2MiB region so the payload can't be larger than the 2MiB alignment.
    let size = helpers::align_up(base, helpers::SIZE_2MB).unwrap() - base;

    // SAFETY: This region is mapped and the linker script prevents it from overlapping with other
    // objects.
    unsafe { slice::from_raw_parts_mut(base as *mut u8, size) }
}

enum AppendedConfigType {
    Valid,
    Invalid,
    NotFound,
}

enum AppendedPayload<'a> {
    /// Configuration data.
    Config(config::Config<'a>),
    /// Deprecated raw BCC, as used in Android T.
    LegacyBcc(&'a mut [u8]),
}

impl<'a> AppendedPayload<'a> {
    /// SAFETY - 'data' should respect the alignment of config::Header.
    unsafe fn new(data: &'a mut [u8]) -> Option<Self> {
        // Safety: This fn has the same constraint as us.
        match unsafe { Self::guess_config_type(data) } {
            AppendedConfigType::Valid => {
                // Safety: This fn has the same constraint as us.
                let config = unsafe { config::Config::new(data) };
                Some(Self::Config(config.unwrap()))
            }
            AppendedConfigType::NotFound if cfg!(feature = "legacy") => {
                const BCC_SIZE: usize = helpers::SIZE_4KB;
                warn!("Assuming the appended data at {:?} to be a raw BCC", data.as_ptr());
                Some(Self::LegacyBcc(&mut data[..BCC_SIZE]))
            }
            _ => None,
        }
    }

    /// SAFETY - 'data' should respect the alignment of config::Header.
    unsafe fn guess_config_type(data: &mut [u8]) -> AppendedConfigType {
        // This function is necessary to prevent the borrow checker from getting confused
        // about the ownership of data in new(); see https://users.rust-lang.org/t/78467.
        let addr = data.as_ptr();

        // Safety: This fn has the same constraint as us.
        match unsafe { config::Config::new(data) } {
            Err(config::Error::InvalidMagic) => {
                warn!("No configuration data found at {addr:?}");
                AppendedConfigType::NotFound
            }
            Err(e) => {
                error!("Invalid configuration data at {addr:?}: {e}");
                AppendedConfigType::Invalid
            }
            Ok(_) => AppendedConfigType::Valid,
        }
    }

    fn get_entries(&mut self) -> (&mut [u8], Option<&mut [u8]>) {
        match self {
            Self::Config(ref mut cfg) => cfg.get_entries(),
            Self::LegacyBcc(ref mut bcc) => (bcc, None),
        }
    }
}
