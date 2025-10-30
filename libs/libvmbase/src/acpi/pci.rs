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

//! Library for accessing ACPI tables

use super::handler::AcpiHandler;
use crate::virtio::pci::{self, PciInfo, PciInfoType};
use acpi::{rsdp::Rsdp, sdt::mcfg, AcpiError, AcpiTables};
use log::debug;
use virtio_drivers::transport::pci::bus::{ConfigurationAccess, PciRoot};

/// An error while constructing PciRoot with information from acpi.
#[derive(Clone, Debug)]
pub enum PciError {
    /// Acpi error while reading ecam range
    AcpiError(AcpiError),
    /// Error constructing PciRoot from ecam range
    VirtioPciError(pci::PciError),
}

impl From<AcpiError> for PciError {
    fn from(e: AcpiError) -> Self {
        Self::AcpiError(e)
    }
}

/// Retrieves ECAM range from MCFG Table
pub fn initialize_from_acpi() -> Result<PciRoot<impl ConfigurationAccess>, PciError> {
    // TODO(b/456796553): Acpi tables are constructed and placed in VM memory by VMM(crosvm)
    // which is untrusted. We should be validating the data before using it.

    // Get the RSDP address
    // TODO: This doesn't work on UEFI. We should revisit if/when UEFI is supported.
    // SAFETY: Crosvm emulates BIOS system and this api is safe on BIOS system.
    let rsdp_addr = unsafe { Rsdp::search_for_on_bios(AcpiHandler)? }.physical_start;

    // SAFETY: search_for_on_bios guarantees that rsdp_addr is valid.
    let tables = unsafe { AcpiTables::from_rsdp(AcpiHandler, rsdp_addr)? };

    // Find the MCFG (PCI Express Memory Mapped Configuration Space Base Address Description Table)
    let mcfg = tables.find_table::<mcfg::Mcfg>().unwrap();

    // Get the first ECAM region's base address.
    let entry = mcfg.entries().first().unwrap();
    // 1MB per bus
    let entry_size = (1 + entry.bus_number_end as usize - entry.bus_number_start as usize) << 20;
    let entry_base = entry.base_address as usize;
    debug!(
        "ECAM: base: {:#x?}, size: {:#x}, bust_start: {}, bus_end: {}",
        entry_base, entry_size, entry.bus_number_start, entry.bus_number_end
    );

    let pci_info = PciInfo { cam_range: entry_base..entry_base + entry_size, bar_range: None };
    pci::initialize(PciInfoType::HypPciInfo(pci_info), None).map_err(PciError::VirtioPciError)
}
