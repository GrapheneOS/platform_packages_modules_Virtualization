// Copyright 2025, The Android Open Source Project
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

//! Uart driver with backend for x86_64 using I/O ports

use crate::arch::x86_64::port;
use crate::uart::UartBackend;

/// Alias for default Uart for x86_64 backend with [`PortBackend`]
pub type Uart = crate::uart::Uart<PortBackend>;

/// Backend for [`crate::uart::Uart`] that uses [`Port`] for writing to hardware registers.
pub struct PortBackend {
    base_port: u16,
}

impl UartBackend for PortBackend {
    fn write_register_u8(&mut self, offset: usize, byte: u8) {
        assert!(offset < 8, "Register offset out of bounds");
        let port = self.base_port + offset as u16;

        // SAFETY: Caller of PortBackend::new() is responsible for providing a valid base port.
        unsafe { port::write_u8(port, byte) };
    }
}

impl Uart {
    /// Constructs a new instance of the UART driver for a device at the given base I/O port.
    ///
    /// # Safety
    ///
    /// The given base I/O port must point to the 8 control registers of an appropriate 8250 UART
    /// device.
    pub unsafe fn new(base_port: u16) -> Self {
        Self::create(PortBackend { base_port })
    }
}
