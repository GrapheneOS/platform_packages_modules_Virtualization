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

use crate::arch::platform::{self, emergency_uart, DEFAULT_EMERGENCY_CONSOLE_INDEX};
use crate::power::reboot;
use core::fmt::{self, write, Arguments, Write};
use core::panic::PanicInfo;

/// Writes a formatted string followed by a newline to the n-th console.
///
/// Returns an error if the n-th console was not initialized by calling [`init`] first.
pub fn writeln(n: usize, format_args: Arguments) -> fmt::Result {
    let uart = &mut *platform::uart(n).ok_or(fmt::Error)?.lock();

    write(uart, format_args)?;
    uart.write_str("\n")?;
    Ok(())
}

/// Prints the given formatted string to the n-th console, followed by a newline.
///
/// Returns an error if the console has not yet been initialized. May deadlock if used in a
/// synchronous exception handler.
#[macro_export]
macro_rules! console_writeln {
    ($n:expr, $($arg:tt)*) => ({
        $crate::console::writeln($n, format_args!($($arg)*))
    })
}

/// Prints the given formatted string to the console, followed by a newline.
///
/// Panics if the console has not yet been initialized. May hang if used in an exception context.
macro_rules! println {
    ($($arg:tt)*) => ({
        $crate::console_writeln!($crate::arch::platform::DEFAULT_CONSOLE_INDEX, $($arg)*).unwrap()
    })
}

pub(crate) use println; // Make it available in this crate.

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // SAFETY: We always reboot at the end of this method so there is no way for the
    // original UART driver to be used after this.
    if let Some(mut console) = unsafe { emergency_uart(DEFAULT_EMERGENCY_CONSOLE_INDEX) } {
        // Ignore errors, to avoid a panic loop.
        let _ = writeln!(console, "{}", info);
    }
    reboot()
}
