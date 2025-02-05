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

//! Minimal driver for an 8250 UART. This only implements enough to work with the emulated 8250
//! provided by crosvm, and won't work with real hardware.

use core::fmt::{self, Write};

/// The backend for [`Uart`] that abstracts the access to 8250 register map
pub trait UartBackend {
    /// Writes a byte value on the offset to the hardware registers.
    fn write_register_u8(&self, offset: usize, byte: u8);
}

/// Minimal driver for an 8250 UART. This only implements enough to work with the emulated 8250
/// provided by crosvm, and won't work with real hardware.
pub struct Uart<Backend: UartBackend> {
    backend: Backend,
}

impl<Backend: UartBackend> Uart<Backend> {
    /// Constructs a new instance of the UART driver with given backend.
    pub(crate) fn create(backend: Backend) -> Self {
        Self { backend }
    }

    /// Writes a single byte to the UART.
    pub fn write_byte(&self, byte: u8) {
        self.backend.write_register_u8(0, byte)
    }
}

impl<Backend: UartBackend> Write for Uart<Backend> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.as_bytes() {
            self.write_byte(*c);
        }
        Ok(())
    }
}
