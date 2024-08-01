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

//! Library for working with (VirtIO) PCI devices discovered from a device tree.

#![no_std]

use core::{
    ffi::CStr,
    fmt::{self, Display, Formatter},
    ops::Range,
};
use libfdt::{AddressRange, Fdt, FdtError, FdtNode};
use log::debug;
use virtio_drivers::transport::pci::bus::{Cam, PciRoot};

/// PCI MMIO configuration region size.
const PCI_CFG_SIZE: usize = 0x100_0000;

/// An error parsing a PCI node from an FDT.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PciError {
    /// Error getting PCI node from FDT.
    FdtErrorPci(FdtError),
    /// Failed to find PCI bus in FDT.
    FdtNoPci,
    /// Error getting `reg` property from PCI node.
    FdtErrorReg(FdtError),
    /// PCI node missing `reg` property.
    FdtMissingReg,
    /// Empty `reg property on PCI node.
    FdtRegEmpty,
    /// PCI `reg` property missing size.
    FdtRegMissingSize,
    /// PCI CAM size reported by FDT is not what we expected.
    CamWrongSize(usize),
    /// Error getting `ranges` property from PCI node.
    FdtErrorRanges(FdtError),
    /// PCI node missing `ranges` property.
    FdtMissingRanges,
    /// Bus address is not equal to CPU physical address in `ranges` property.
    RangeAddressMismatch {
        /// A bus address from the `ranges` property.
        bus_address: u64,
        /// The corresponding CPU physical address from the `ranges` property.
        cpu_physical: u64,
    },
    /// No suitable PCI memory range found.
    NoSuitableRange,
}

impl Display for PciError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::FdtErrorPci(e) => write!(f, "Error getting PCI node from FDT: {}", e),
            Self::FdtNoPci => write!(f, "Failed to find PCI bus in FDT."),
            Self::FdtErrorReg(e) => write!(f, "Error getting reg property from PCI node: {}", e),
            Self::FdtMissingReg => write!(f, "PCI node missing reg property."),
            Self::FdtRegEmpty => write!(f, "Empty reg property on PCI node."),
            Self::FdtRegMissingSize => write!(f, "PCI reg property missing size."),
            Self::CamWrongSize(cam_size) => write!(
                f,
                "FDT says PCI CAM is {} bytes but we expected {}.",
                cam_size, PCI_CFG_SIZE
            ),
            Self::FdtErrorRanges(e) => {
                write!(f, "Error getting ranges property from PCI node: {}", e)
            }
            Self::FdtMissingRanges => write!(f, "PCI node missing ranges property."),
            Self::RangeAddressMismatch { bus_address, cpu_physical } => {
                write!(
                    f,
                    "bus address {:#018x} != CPU physical address {:#018x}",
                    bus_address, cpu_physical
                )
            }
            Self::NoSuitableRange => write!(f, "No suitable PCI memory range found."),
        }
    }
}

/// Information about the PCI bus parsed from the device tree.
#[derive(Clone, Debug)]
pub struct PciInfo {
    /// The MMIO range used by the memory-mapped PCI CAM.
    pub cam_range: Range<usize>,
    /// The MMIO range from which 32-bit PCI BARs should be allocated.
    pub bar_range: Range<u32>,
}

impl PciInfo {
    /// Finds the PCI node in the FDT, parses its properties and validates it.
    pub fn from_fdt(fdt: &Fdt) -> Result<Self, PciError> {
        let pci_node = pci_node(fdt)?;

        let cam_range = parse_cam_range(&pci_node)?;
        let bar_range = parse_ranges(&pci_node)?;

        Ok(Self { cam_range, bar_range })
    }

    /// Returns the `PciRoot` for the memory-mapped CAM found in the FDT. The CAM should be mapped
    /// before this is called, by calling [`PciInfo::map`].
    ///
    /// # Safety
    ///
    /// To prevent concurrent access, only one `PciRoot` should exist in the program. Thus this
    /// method must only be called once, and there must be no other `PciRoot` constructed using the
    /// same CAM.
    pub unsafe fn make_pci_root(&self) -> PciRoot {
        // SAFETY: We trust that the FDT gave us a valid MMIO base address for the CAM. The caller
        // guarantees to only call us once, so there are no other references to it.
        unsafe { PciRoot::new(self.cam_range.start as *mut u8, Cam::MmioCam) }
    }
}

/// Finds an FDT node with compatible=pci-host-cam-generic.
fn pci_node(fdt: &Fdt) -> Result<FdtNode, PciError> {
    fdt.compatible_nodes(CStr::from_bytes_with_nul(b"pci-host-cam-generic\0").unwrap())
        .map_err(PciError::FdtErrorPci)?
        .next()
        .ok_or(PciError::FdtNoPci)
}

/// Parses the "reg" property of the given PCI FDT node to find the MMIO CAM range.
fn parse_cam_range(pci_node: &FdtNode) -> Result<Range<usize>, PciError> {
    let pci_reg = pci_node
        .reg()
        .map_err(PciError::FdtErrorReg)?
        .ok_or(PciError::FdtMissingReg)?
        .next()
        .ok_or(PciError::FdtRegEmpty)?;
    let cam_addr = pci_reg.addr as usize;
    let cam_size = pci_reg.size.ok_or(PciError::FdtRegMissingSize)? as usize;
    debug!("Found PCI CAM at {:#x}-{:#x}", cam_addr, cam_addr + cam_size);
    // Check that the CAM is the size we expect, so we don't later try accessing it beyond its
    // bounds. If it is a different size then something is very wrong and we shouldn't continue to
    // access it; maybe there is some new version of PCI we don't know about.
    if cam_size != PCI_CFG_SIZE {
        return Err(PciError::CamWrongSize(cam_size));
    }

    Ok(cam_addr..cam_addr + cam_size)
}

/// Parses the "ranges" property of the given PCI FDT node, and returns the largest suitable range
/// to use for non-prefetchable 32-bit memory BARs.
fn parse_ranges(pci_node: &FdtNode) -> Result<Range<u32>, PciError> {
    let mut memory_address = 0;
    let mut memory_size = 0;

    for AddressRange { addr: (flags, bus_address), parent_addr: cpu_physical, size } in pci_node
        .ranges::<(u32, u64), u64, u64>()
        .map_err(PciError::FdtErrorRanges)?
        .ok_or(PciError::FdtMissingRanges)?
    {
        let flags = PciMemoryFlags(flags);
        let prefetchable = flags.prefetchable();
        let range_type = flags.range_type();
        debug!(
            "range: {:?} {}prefetchable bus address: {:#018x} CPU physical address: {:#018x} size: {:#018x}",
            range_type,
            if prefetchable { "" } else { "non-" },
            bus_address,
            cpu_physical,
            size,
        );

        // Use a 64-bit range for 32-bit memory, if it is low enough, because crosvm doesn't
        // currently provide any 32-bit ranges.
        if !prefetchable
            && matches!(range_type, PciRangeType::Memory32 | PciRangeType::Memory64)
            && size > memory_size.into()
            && bus_address + size < u32::MAX.into()
        {
            if bus_address != cpu_physical {
                return Err(PciError::RangeAddressMismatch { bus_address, cpu_physical });
            }
            memory_address = u32::try_from(cpu_physical).unwrap();
            memory_size = u32::try_from(size).unwrap();
        }
    }

    if memory_size == 0 {
        return Err(PciError::NoSuitableRange);
    }

    Ok(memory_address..memory_address + memory_size)
}

/// Encodes memory flags of a PCI range
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct PciMemoryFlags(pub u32);

impl PciMemoryFlags {
    /// Returns whether this PCI range is prefetchable
    pub fn prefetchable(self) -> bool {
        self.0 & 0x40000000 != 0
    }

    /// Returns the type of this PCI range
    pub fn range_type(self) -> PciRangeType {
        PciRangeType::from((self.0 & 0x3000000) >> 24)
    }
}

/// Type of a PCI range
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PciRangeType {
    /// Range represents the PCI configuration space
    ConfigurationSpace,
    /// Range is on IO space
    IoSpace,
    /// Range is on 32-bit MMIO space
    Memory32,
    /// Range is on 64-bit MMIO space
    Memory64,
}

impl From<u32> for PciRangeType {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::ConfigurationSpace,
            1 => Self::IoSpace,
            2 => Self::Memory32,
            3 => Self::Memory64,
            _ => panic!("Tried to convert invalid range type {}", value),
        }
    }
}
