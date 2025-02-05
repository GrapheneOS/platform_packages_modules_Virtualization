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

use crate::arch::payload::jump_to_payload;
use crate::config;
use crate::memory::MemorySlices;
use core::slice;
use log::error;
use log::warn;
use log::LevelFilter;
use vmbase::{
    configure_heap, console_writeln, limit_stack_size, main,
    memory::{
        map_image_footer, unshare_all_memory, unshare_all_mmio_except_uart, unshare_uart,
        MemoryTrackerError, SIZE_128KB, SIZE_4KB,
    },
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
limit_stack_size!(SIZE_4KB * 12);

#[derive(Debug)]
enum NextStage {
    LinuxBoot(usize),
    LinuxBootWithUart(usize),
}

/// Entry point for pVM firmware.
pub fn start(fdt_address: u64, payload_start: u64, payload_size: u64, _arg3: u64) {
    let fdt_address = fdt_address.try_into().unwrap();
    let payload_start = payload_start.try_into().unwrap();
    let payload_size = payload_size.try_into().unwrap();

    let reboot_reason = match main_wrapper(fdt_address, payload_start, payload_size) {
        Err(r) => r,
        Ok((next_stage, slices)) => match next_stage {
            NextStage::LinuxBootWithUart(ep) => jump_to_payload(ep, &slices),
            NextStage::LinuxBoot(ep) => {
                if let Err(e) = unshare_uart() {
                    error!("Failed to unmap UART: {e}");
                    RebootReason::InternalError
                } else {
                    jump_to_payload(ep, &slices)
                }
            }
        },
    };

    const REBOOT_REASON_CONSOLE: usize = 1;
    console_writeln!(REBOOT_REASON_CONSOLE, "{}", reboot_reason.as_avf_reboot_string());
    reboot()

    // if we reach this point and return, vmbase::entry::rust_entry() will call power::shutdown().
}

/// Sets up the environment for main() and wraps its result for start().
///
/// Provide the abstractions necessary for start() to abort the pVM boot and for main() to run with
/// the assumption that its environment has been properly configured.
fn main_wrapper<'a>(
    fdt: usize,
    payload: usize,
    payload_size: usize,
) -> Result<(NextStage, MemorySlices<'a>), RebootReason> {
    // Limitations in this function:
    // - only access MMIO once (and while) it has been mapped and configured
    // - only perform logging once the logger has been initialized
    // - only access non-pvmfw memory once (and while) it has been mapped

    log::set_max_level(LevelFilter::Info);

    let appended_data = get_appended_data_slice().map_err(|e| {
        error!("Failed to map the appended data: {e}");
        RebootReason::InternalError
    })?;

    let appended = AppendedPayload::new(appended_data).ok_or_else(|| {
        error!("No valid configuration found");
        RebootReason::InvalidConfig
    })?;

    let config_entries = appended.get_entries();

    let mut slices = MemorySlices::new(fdt, payload, payload_size)?;

    // This wrapper allows main() to be blissfully ignorant of platform details.
    let (next_bcc, debuggable_payload) = crate::main(
        slices.fdt,
        slices.kernel,
        slices.ramdisk,
        config_entries.bcc,
        config_entries.debug_policy,
        config_entries.vm_dtbo,
        config_entries.vm_ref_dt,
    )?;
    slices.add_dice_chain(next_bcc);
    // Keep UART MMIO_GUARD-ed for debuggable payloads, to enable earlycon.
    let keep_uart = cfg!(debuggable_vms_improvements) && debuggable_payload;

    // Writable-dirty regions will be flushed when MemoryTracker is dropped.
    config_entries.bcc.zeroize();

    unshare_all_mmio_except_uart().map_err(|e| {
        error!("Failed to unshare MMIO ranges: {e}");
        RebootReason::InternalError
    })?;
    unshare_all_memory();

    let next_stage = select_next_stage(slices.kernel, keep_uart);

    Ok((next_stage, slices))
}

fn select_next_stage(kernel: &[u8], keep_uart: bool) -> NextStage {
    if keep_uart {
        NextStage::LinuxBootWithUart(kernel.as_ptr() as _)
    } else {
        NextStage::LinuxBoot(kernel.as_ptr() as _)
    }
}

fn get_appended_data_slice() -> Result<&'static mut [u8], MemoryTrackerError> {
    let range = map_image_footer()?;
    // SAFETY: This region was just mapped for the first time (as map_image_footer() didn't fail)
    // and the linker script prevents it from overlapping with other objects.
    Ok(unsafe { slice::from_raw_parts_mut(range.start as *mut u8, range.len()) })
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
