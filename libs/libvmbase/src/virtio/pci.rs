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

//! Functions to scan the PCI bus for VirtIO devices.

use crate::memory::{MemoryTracker, MemoryTrackerError};
use alloc::boxed::Box;
use core::fmt;
use core::marker::PhantomData;
use fdtpci::PciInfo;
use log::debug;
use once_cell::race::OnceBox;
use virtio_drivers::{
    device::{blk, socket},
    transport::pci::{
        bus::{BusDeviceIterator, PciRoot},
        virtio_device_type, PciTransport,
    },
    Hal,
};

pub(super) static PCI_INFO: OnceBox<PciInfo> = OnceBox::new();

/// PCI errors.
#[derive(Debug, Clone)]
pub enum PciError {
    /// Attempted to initialize the PCI more than once.
    DuplicateInitialization,
    /// Failed to map PCI CAM.
    CamMapFailed(MemoryTrackerError),
    /// Failed to map PCI BAR.
    BarMapFailed(MemoryTrackerError),
}

impl fmt::Display for PciError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::DuplicateInitialization => {
                write!(f, "Attempted to initialize the PCI more than once.")
            }
            Self::CamMapFailed(e) => write!(f, "Failed to map PCI CAM: {e}"),
            Self::BarMapFailed(e) => write!(f, "Failed to map PCI BAR: {e}"),
        }
    }
}

/// Prepares to use VirtIO PCI devices.
///
/// In particular:
///
/// 1. Maps the PCI CAM and BAR range in the page table and MMIO guard.
/// 2. Stores the `PciInfo` for the VirtIO HAL to use later.
/// 3. Creates and returns a `PciRoot`.
///
/// This must only be called once; it will panic if it is called a second time.
pub fn initialize(pci_info: PciInfo, memory: &mut MemoryTracker) -> Result<PciRoot, PciError> {
    PCI_INFO.set(Box::new(pci_info.clone())).map_err(|_| PciError::DuplicateInitialization)?;

    memory.map_mmio_range(pci_info.cam_range.clone()).map_err(PciError::CamMapFailed)?;
    let bar_range = pci_info.bar_range.start as usize..pci_info.bar_range.end as usize;
    memory.map_mmio_range(bar_range).map_err(PciError::BarMapFailed)?;

    // Safety: This is the only place where we call make_pci_root, and `PCI_INFO.set` above will
    // panic if it is called a second time.
    Ok(unsafe { pci_info.make_pci_root() })
}

/// Virtio Block device.
pub type VirtIOBlk<T> = blk::VirtIOBlk<T, PciTransport>;

/// Virtio Socket device.
///
/// Spec: https://docs.oasis-open.org/virtio/virtio/v1.2/csd01/virtio-v1.2-csd01.html 5.10
pub type VirtIOSocket<T> = socket::VirtIOSocket<T, PciTransport>;

/// An iterator that iterates over the PCI transport for each device.
pub struct PciTransportIterator<'a, T: Hal> {
    pci_root: &'a mut PciRoot,
    bus: BusDeviceIterator,
    _hal: PhantomData<T>,
}

impl<'a, T: Hal> PciTransportIterator<'a, T> {
    /// Creates a new iterator.
    pub fn new(pci_root: &'a mut PciRoot) -> Self {
        let bus = pci_root.enumerate_bus(0);
        Self { pci_root, bus, _hal: PhantomData }
    }
}

impl<'a, T: Hal> Iterator for PciTransportIterator<'a, T> {
    type Item = PciTransport;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (device_function, info) = self.bus.next()?;
            let (status, command) = self.pci_root.get_status_command(device_function);
            debug!(
                "Found PCI device {} at {}, status {:?} command {:?}",
                info, device_function, status, command
            );

            let Some(virtio_type) = virtio_device_type(&info) else {
                continue;
            };
            debug!("  VirtIO {:?}", virtio_type);

            return PciTransport::new::<T>(self.pci_root, device_function).ok();
        }
    }
}
