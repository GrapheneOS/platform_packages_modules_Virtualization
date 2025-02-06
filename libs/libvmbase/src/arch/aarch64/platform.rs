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

//! Definition of platform

use crate::{
    arch::aarch64::{
        layout::{UART_ADDRESSES, UART_PAGE_ADDR},
        uart::Uart,
    },
    memory::{SIZE_16KB, SIZE_4KB},
};
use smccc::{
    psci::{system_off, system_reset},
    Hvc,
};
use spin::{mutex::SpinMutex, once::Once};
use static_assertions::const_assert_eq;

// Arbitrary limit on the number of consoles that can be registered.
//
// Matches the UART count in crosvm.
const MAX_CONSOLES: usize = 4;

static CONSOLES: [Once<SpinMutex<Uart>>; MAX_CONSOLES] =
    [Once::new(), Once::new(), Once::new(), Once::new()];
static ADDRESSES: [Once<usize>; MAX_CONSOLES] =
    [Once::new(), Once::new(), Once::new(), Once::new()];

/// Index of the console used by default for logging.
pub const DEFAULT_CONSOLE_INDEX: usize = 0;

/// Index of the console used by default for emergency logging.
pub const DEFAULT_EMERGENCY_CONSOLE_INDEX: usize = DEFAULT_CONSOLE_INDEX;

/// Initialises the global instance(s) of the UART driver.
///
/// # Safety
///
/// This must be called before using the `print!` and `println!` macros.
/// The only safe place to execute this function is in rust initialization code.
///
/// This must be called once with the bases of UARTs, mapped as device memory and (if necessary)
/// shared with the host as MMIO, to which no other references must be held.
pub unsafe fn init_all_uart(base_addresses: &[usize]) {
    for (i, &base_address) in base_addresses.iter().enumerate() {
        // Remember the valid address, for emergency console accesses.
        ADDRESSES[i].call_once(|| base_address);

        // Initialize the console driver, for normal console accesses.
        assert!(!CONSOLES[i].is_completed(), "console::init() called more than once");
        // SAFETY: The caller promised that base_address is the base of a mapped UART with no
        // aliases.
        CONSOLES[i].call_once(|| SpinMutex::new(unsafe { Uart::new(base_address) }));
    }
}

/// Initialize console by mapping MMIO memory
pub fn map_uarts_mmio() -> Result<(), hypervisor_backends::Error> {
    if let Some(mmio_guard) = hypervisor_backends::get_mmio_guard() {
        mmio_guard.enroll()?;

        // TODO(ptosi): Use MmioSharer::share() to properly track this MMIO_GUARD_MAP.
        //
        // The following call shares the UART but also anything else present in 0..granule.
        //
        // For 4KiB, that's only the UARTs. For 16KiB, it also covers the RTC and watchdog but, as
        // neither is used by vmbase clients (and as both are outside of the UART page), they
        // will never have valid stage-1 mappings to those devices. As a result, this
        // MMIO_GUARD_MAP isn't affected by the granule size in any visible way. Larger granule
        // sizes will need to be checked separately, if needed.
        assert!({
            let granule = mmio_guard.granule()?;
            granule == SIZE_4KB || granule == SIZE_16KB
        });
        // Validate the assumption above by ensuring that the UART is not moved to another page:
        const_assert_eq!(UART_PAGE_ADDR, 0);
        mmio_guard.map(UART_PAGE_ADDR)?;
    }
    Ok(())
}

/// Initialize platform specific device drivers. If this function fails the reboot is issued.
pub fn init_console() {
    if map_uarts_mmio().is_err() {
        // UART mapping failed platform can't provide any output.
        // Reboot to prevent printing any message.
        reboot()
    }
    // SAFETY: UART_PAGE is mapped at stage-1 (see entry.S) and was just MMIO-guarded.
    unsafe { init_all_uart(&UART_ADDRESSES) };
}

/// Return platform uart with specific index
///
/// Panics if console was not initialized by calling [`init`] first.
pub fn uart(id: usize) -> &'static spin::mutex::SpinMutex<Uart> {
    CONSOLES[id].get().unwrap()
}

/// Reinitializes the emergency UART driver and returns it.
///
/// This is intended for use in situations where the UART may be in an unknown state or the global
/// instance may be locked, such as in an exception handler or panic handler.
pub fn emergency_uart() -> Uart {
    // SAFETY: Initialization of UART using dedicated const address.
    unsafe { Uart::new(UART_ADDRESSES[DEFAULT_EMERGENCY_CONSOLE_INDEX]) }
}

/// Makes a `PSCI_SYSTEM_OFF` call to shutdown the VM.
///
/// Panics if it returns an error.
pub fn shutdown() -> ! {
    system_off::<Hvc>().unwrap();
    #[allow(clippy::empty_loop)]
    loop {}
}

/// Makes a `PSCI_SYSTEM_RESET` call to shutdown the VM abnormally.
///
/// Panics if it returns an error.
pub fn reboot() -> ! {
    system_reset::<Hvc>().unwrap();
    #[allow(clippy::empty_loop)]
    loop {}
}
