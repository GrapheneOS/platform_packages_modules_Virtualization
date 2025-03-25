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

//! Uart driver with backend for aarch64 using MMIO

use crate::uart::UartBackend;
use core::ptr::NonNull;
use safe_mmio::{fields::ReadWrite, UniqueMmioPointer};

/// Alias for default Uart for aarch64 backend with [`MmioBackend`]
pub type Uart = crate::uart::Uart<MmioBackend<'static>>;

/// Backend for [`crate::uart::Uart`] that uses `safe-mmio` for writing to hardware registers.
pub struct MmioBackend<'a> {
    registers: UniqueMmioPointer<'a, [ReadWrite<u8>; 8]>,
}

impl<'a> MmioBackend<'a> {
    /// Constructs a new instance of the UART driver backend for a device with the given data
    /// register.
    pub fn new(registers: UniqueMmioPointer<'a, [ReadWrite<u8>; 8]>) -> Self {
        Self { registers }
    }
}

impl UartBackend for MmioBackend<'_> {
    fn write_register_u8(&mut self, offset: usize, byte: u8) {
        self.registers.get(offset).expect("Register offset out of bounds").write(byte);
    }
}

impl Uart {
    /// Constructs a new instance of the UART driver for a device at the given base address.
    ///
    /// # Safety
    ///
    /// The given base address must point to the 8 MMIO control registers of an appropriate 8250
    /// UART device, which must be mapped into the address space of the process as device memory and
    /// not have any other aliases.
    pub unsafe fn new(base_address: usize) -> Self {
        // SAFETY: The caller promises that base_address points to a mapped 8250 UART's registers
        // with no aliases. That implies that there are 8 single-byte registers we can safely
        // access, as required by `UniqueMmioPointer`.
        let data_register =
            unsafe { UniqueMmioPointer::new(NonNull::new(base_address as _).unwrap()) };
        Self::create(MmioBackend::new(data_register))
    }
}
