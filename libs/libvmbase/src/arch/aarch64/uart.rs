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

use crate::arch::write_volatile_u8;
use crate::uart::UartBackend;

/// Alias for default Uart for aarch64 backend with [`MmioBackend`]
pub type Uart = crate::uart::Uart<MmioBackend>;

/// Backend for [`crate::uart::Uart`] that uses [`crate::arch::write_volatile_u8`] for writing to
/// hardware registers.
pub struct MmioBackend {
    base_address: *mut u8,
}

impl MmioBackend {
    /// Constructs a new instance of the UART driver backend for a device at the given base address.
    ///
    /// # Safety
    ///
    /// The given base address must point to the 8 MMIO control registers of an appropriate UART
    /// device, which must be mapped into the address space of the process as device memory and not
    /// have any other aliases.
    pub unsafe fn new(base_address: usize) -> Self {
        Self { base_address: base_address as *mut u8 }
    }
}

impl UartBackend for MmioBackend {
    fn write_register_u8(&self, offset: usize, byte: u8) {
        // SAFETY: We know that the base address points to the control registers of a UART device
        // which is appropriately mapped.
        unsafe { write_volatile_u8(self.base_address.add(offset), byte) }
    }
}

impl Uart {
    /// Constructs a new instance of the UART driver for a device at the given base address.
    ///
    /// # Safety
    ///
    /// The given base address must point to the 8 MMIO control registers of an appropriate UART
    /// device, which must be mapped into the address space of the process as device memory and not
    /// have any other aliases.
    pub unsafe fn new(base_address: usize) -> Self {
        // SAFETY: Delegated to caller
        unsafe { Self::create(MmioBackend::new(base_address)) }
    }
}

// SAFETY: `MmioBackend` just contains a pointer to device memory, which can be accessed from any
// context.
unsafe impl Send for MmioBackend {}
