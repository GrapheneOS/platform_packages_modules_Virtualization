// Copyright 2024, The Android Open Source Project
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

//! Platform initialization

use crate::arch::x86_64::{layout::UART_PORTS, uart::Uart};
use core::mem::MaybeUninit;
use spin::{mutex::SpinMutex, once::Once};

// Arbitrary limit on the number of consoles that can be registered.
//
// Matches the UART count in crosvm.
const MAX_CONSOLES: usize = 4;

static CONSOLES: [Once<SpinMutex<Uart>>; MAX_CONSOLES] =
    [Once::new(), Once::new(), Once::new(), Once::new()];
static PORTS: [Once<u16>; MAX_CONSOLES] = [Once::new(), Once::new(), Once::new(), Once::new()];

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
/// This must be called once with the I/O ports of UARTs.
pub unsafe fn init_all_uart(ports: &[u16]) {
    for (i, &base_port) in ports.iter().enumerate() {
        // Remember the valid port, for emergency console accesses.
        PORTS[i].call_once(|| base_port);

        // Initialize the console driver, for normal console accesses.
        assert!(!CONSOLES[i].is_completed(), "console::init() called more than once");
        // SAFETY: The caller promised that base_port is the base of a UART.
        CONSOLES[i].call_once(|| SpinMutex::new(unsafe { Uart::new(base_port) }));
    }
}

/// Initialize platform specific device drivers.
pub fn init_console() {
    // SAFETY: UART_PORTS are known to be valid UART I/O port numbers.
    unsafe { init_all_uart(&UART_PORTS) };
}

/// Return platform uart with specific index
///
/// Returns `None` if console was not initialized by calling [`init`] first.
pub fn uart(id: usize) -> Option<&'static SpinMutex<Uart>> {
    CONSOLES.get(id)?.get()
}

/// Reinitializes the n-th UART driver and returns it.
///
/// This is intended for use in situations where the UART may be in an unknown state or the global
/// instance may be locked, such as in the synchronous exception handler.
///
/// # Safety
///
/// This takes over the UART from wherever it is being used, the existing UART instance should not
/// be used after this is called. This should only be used immediately before aborting the VM.
pub unsafe fn emergency_uart(id: usize) -> Option<Uart> {
    let base_port = *PORTS.get(id)?.get()?;

    // SAFETY: Initialization of UART using dedicated const address.
    Some(unsafe { Uart::new(base_port) })
}

/// Makes a call to shutdown the VM.
pub fn shutdown() -> ! {
    // TODO(b/354116267): implement for x86_64
    #[allow(clippy::empty_loop)]
    loop {}
}

/// Makes a hypercall to reboot the VM.
pub fn reboot() -> ! {
    // TODO(b/354116267): implement for x86_64
    #[allow(clippy::empty_loop)]
    loop {}
}
