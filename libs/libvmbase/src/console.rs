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

use crate::arch::platform;
use core::fmt::{write, Arguments, Write};

/// Writes a formatted string followed by a newline to the n-th console.
///
/// Panics if the n-th console was not initialized by calling [`init`] first.
pub fn writeln(n: usize, format_args: Arguments) {
    let uart = &mut *platform::uart(n).lock();
    write(uart, format_args).unwrap();
    let _ = uart.write_str("\n");
}

/// Reinitializes the emergency UART driver and writes a formatted string followed by a newline to
/// it.
///
/// This is intended for use in situations where the UART may be in an unknown state or the global
/// instance may be locked, such as in an exception handler or panic handler.
pub fn ewriteln(format_args: Arguments) {
    let mut uart = platform::emergency_uart();
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
        $crate::console::console_writeln!($crate::arch::platform::DEFAULT_CONSOLE_INDEX, $($arg)*)
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
        $crate::console::ewriteln(format_args!($($arg)*))
    })
}
