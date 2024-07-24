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

//! Console driver for 8250 UART.

use crate::uart::Uart;
use core::fmt::{write, Arguments, Write};
use spin::{mutex::SpinMutex, Once};

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
/// This must be called before using the `print!` and `println!` macros.
///
/// # Safety
///
/// This must be called once with the bases of UARTs, mapped as device memory and (if necessary)
/// shared with the host as MMIO, to which no other references must be held.
pub unsafe fn init(base_addresses: &[usize]) {
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

/// Writes a formatted string followed by a newline to the n-th console.
///
/// Panics if the n-th console was not initialized by calling [`init`] first.
pub fn writeln(n: usize, format_args: Arguments) {
    let uart = &mut *CONSOLES[n].get().unwrap().lock();

    write(uart, format_args).unwrap();
    let _ = uart.write_str("\n");
}

/// Reinitializes the n-th UART driver and writes a formatted string followed by a newline to it.
///
/// This is intended for use in situations where the UART may be in an unknown state or the global
/// instance may be locked, such as in an exception handler or panic handler.
pub fn ewriteln(n: usize, format_args: Arguments) {
    let Some(addr) = ADDRESSES[n].get() else { return };

    // SAFETY: addr contains the base of a mapped UART, passed in init().
    let mut uart = unsafe { Uart::new(*addr) };

    let _ = write(&mut uart, format_args);
    let _ = uart.write_str("\n");
}

/// Prints the given formatted string to the n-th console, followed by a newline.
///
/// Panics if the console has not yet been initialized. May hang if used in an exception context;
/// use `eprintln!` instead.
#[macro_export]
macro_rules! console_writeln {
    ($n:expr, $($arg:tt)*) => ({
        $crate::console::writeln($n, format_args!($($arg)*))
    })
}

pub(crate) use console_writeln;

/// Prints the given formatted string to the console, followed by a newline.
///
/// Panics if the console has not yet been initialized. May hang if used in an exception context;
/// use `eprintln!` instead.
macro_rules! println {
    ($($arg:tt)*) => ({
        $crate::console::console_writeln!($crate::console::DEFAULT_CONSOLE_INDEX, $($arg)*)
    })
}

pub(crate) use println; // Make it available in this crate.

/// Prints the given string followed by a newline to the console in an emergency, such as an
/// exception handler.
///
/// Never panics.
#[macro_export]
macro_rules! eprintln {
    ($($arg:tt)*) => ({
        $crate::console::ewriteln($crate::console::DEFAULT_EMERGENCY_CONSOLE_INDEX, format_args!($($arg)*))
    })
}
