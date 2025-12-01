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

use crate::memory::{init_shared_pool, map_device, MemoryTrackerError};
use alloc::boxed::Box;
use core::marker::PhantomData;
use core::ops::Range;
use log::debug;
use once_cell::race::OnceBox;
#[cfg(not(target_arch = "x86_64"))]
use virtio_drivers::transport::pci::{bus::MmioCam, PciTransport};
#[cfg(target_arch = "x86_64")]
use virtio_drivers::transport::x86_64::{HypCam, HypPciTransport};
use virtio_drivers::{
    device::{blk, socket},
    transport::{
        pci::{
            bus::{BusDeviceIterator, Cam, ConfigurationAccess, DeviceFunction, PciRoot},
            virtio_device_type, VirtioPciError,
        },
        SomeTransport,
    },
    Hal,
};

/// Information about the PCI bus for use by the virtio driver
#[derive(Clone, Debug)]
pub struct PciInfo {
    /// The MMIO range used by the memory-mapped PCI CAM.
    pub cam_range: Range<usize>,
    /// The MMIO range from which 32-bit PCI BARs should be allocated.
    /// Used for validating the address by Hal. Not needed if mmio is
    /// accessed through hypercalls(pkvm).
    pub bar_range: Option<Range<u32>>,
}

impl PciInfo {
    // Returns the `PciRoot` for the memory-mapped CAM. The CAM should be mapped
    // before this is called, by calling [`PciInfo::map`].
    //
    // # Safety
    //
    // To prevent concurrent access, only one `PciRoot` should exist in the program. Thus this
    // method must only be called once, and there must be no other `PciRoot` constructed using the
    // same CAM.
    #[cfg(target_arch = "aarch64")]
    unsafe fn make_pci_root(&self) -> PciRoot<impl ConfigurationAccess> {
        // SAFETY: We trust that the FDT gave us a valid MMIO base address for the CAM. The caller
        // guarantees to only call us once, so there are no other references to it.
        PciRoot::new(unsafe { MmioCam::new(self.cam_range.start as *mut u8, Cam::MmioCam) })
    }

    // Returns the `PciRoot` for the memory-mapped CAM. The CAM should be mapped
    // before this is called, by calling [`PciInfo::map`].
    //
    // # Safety
    //
    // To prevent concurrent access, only one `PciRoot` should exist in the program. Thus this
    // method must only be called once, and there must be no other `PciRoot` constructed using the
    // same CAM.
    #[cfg(target_arch = "x86_64")]
    unsafe fn make_pci_root(&self) -> PciRoot<impl ConfigurationAccess> {
        PciRoot::new(HypCam::new(self.cam_range.start, Cam::Ecam))
    }
}

/// PciInfo types
#[derive(Clone, Debug)]
pub enum PciInfoType {
    /// PciInfo for default MMIO access
    MmioPciInfo(PciInfo),
    /// PciInfo for MMIO access through hypercalls(x86_64 pkvm)
    HypPciInfo(PciInfo),
}

impl PciInfoType {
    /// Returns Some reference to contained PciInfo, if MmioPciInfo
    /// else None
    pub fn mmio_pci_info(&self) -> Option<&PciInfo> {
        if let PciInfoType::MmioPciInfo(pci_info) = self {
            Some(pci_info)
        } else {
            None
        }
    }

    /// Returns Some reference to contained PciInfo, if HypPciInfo
    /// else None
    pub fn hyp_pci_info(&self) -> Option<&PciInfo> {
        if let PciInfoType::HypPciInfo(pci_info) = self {
            Some(pci_info)
        } else {
            None
        }
    }

    /// Returns reference to contained PciInfo
    pub fn pci_info(&self) -> &PciInfo {
        match self {
            PciInfoType::MmioPciInfo(pci_info) => pci_info,
            PciInfoType::HypPciInfo(pci_info) => pci_info,
        }
    }
}

pub(super) static PCI_INFO_TYPE: OnceBox<PciInfoType> = OnceBox::new();

/// PCI errors.
#[derive(Debug, thiserror::Error, Clone)]
pub enum PciError {
    /// Attempted to initialize the PCI more than once.
    #[error("Attempted to initialize the PCI more than once.")]
    DuplicateInitialization,
    /// Failed to map PCI CAM.
    #[error("Failed to map PCI CAM: {0}")]
    CamMapFailed(#[source] MemoryTrackerError),
    /// Failed to map PCI BAR.
    #[error("Failed to map PCI BAR: {0}")]
    BarMapFailed(#[source] MemoryTrackerError),
    /// Failed to initialize shared pool
    #[error("Failed to initialize shared pool: {0}")]
    SharedPoolInitFailed(#[source] MemoryTrackerError),
}

/// Prepares to use VirtIO PCI devices.
///
/// In particular:
///
/// 1. Maps the PCI CAM and BAR range in the page table and MMIO guard.
/// 2. Stores the `PciInfo` for the VirtIO HAL to use later.
/// 3. Creates and returns a `PciRoot`.
///
/// This must only be called once and after having switched to the dynamic page tables.
pub fn initialize(
    pci_info_type: PciInfoType,
    swiotlb_range: Option<&Range<usize>>,
) -> Result<PciRoot<impl ConfigurationAccess>, PciError> {
    PCI_INFO_TYPE
        .set(Box::new(pci_info_type.clone()))
        .map_err(|_| PciError::DuplicateInitialization)?;

    let pci_info_type = PCI_INFO_TYPE.get().unwrap();

    // Map the cam and bar ranges if mmio space is accessed directly.
    // Not needed for hypercall based mmio access.
    if let PciInfoType::MmioPciInfo(pci_info) = pci_info_type {
        let cam_start = pci_info.cam_range.start;
        let cam_size = pci_info.cam_range.len().try_into().unwrap();
        map_device(cam_start, cam_size).map_err(PciError::CamMapFailed)?;

        let bar_range = pci_info.bar_range.as_ref().unwrap();
        let bar_start = bar_range.start.try_into().unwrap();
        let bar_size = bar_range.len().try_into().unwrap();
        map_device(bar_start, bar_size).map_err(PciError::BarMapFailed)?;
    }

    init_shared_pool(swiotlb_range).map_err(PciError::SharedPoolInitFailed)?;

    // SAFETY: This is the only place where we call make_pci_root, validated by `PCI_INFO_TYPE.set`.
    Ok(unsafe { pci_info_type.pci_info().make_pci_root() })
}

/// Virtio Block device.
pub type VirtIOBlk<'a, T> = blk::VirtIOBlk<T, SomeTransport<'a>>;

/// Virtio Socket device.
///
/// Spec: https://docs.oasis-open.org/virtio/virtio/v1.2/csd01/virtio-v1.2-csd01.html 5.10
pub type VirtIOSocket<'a, T> = socket::VirtIOSocket<T, SomeTransport<'a>>;

/// An iterator that iterates over the PCI transport for each device.
pub struct PciTransportIterator<'a, T: Hal, C: ConfigurationAccess> {
    pci_root: &'a mut PciRoot<C>,
    bus: BusDeviceIterator<C>,
    _hal: PhantomData<T>,
}

impl<'a, T: Hal, C: ConfigurationAccess> PciTransportIterator<'a, T, C> {
    /// Creates a new iterator.
    pub fn new(pci_root: &'a mut PciRoot<C>) -> Self {
        let bus = pci_root.enumerate_bus(0);
        Self { pci_root, bus, _hal: PhantomData }
    }

    #[cfg(target_arch = "aarch64")]
    fn pci_transport(
        &mut self,
        device_function: DeviceFunction,
    ) -> Result<SomeTransport<'a>, VirtioPciError> {
        PciTransport::new::<T, C>(self.pci_root, device_function).map(SomeTransport::Pci)
    }

    #[cfg(target_arch = "x86_64")]
    fn pci_transport(
        &mut self,
        device_function: DeviceFunction,
    ) -> Result<SomeTransport<'a>, VirtioPciError> {
        HypPciTransport::new::<C>(self.pci_root, device_function).map(SomeTransport::HypPci)
    }
}

impl<'a, T: Hal, C: ConfigurationAccess> Iterator for PciTransportIterator<'a, T, C> {
    type Item = SomeTransport<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (device_function, info) = self.bus.next()?;
            let (status, command) = self.pci_root.get_status_command(device_function);
            debug!(
                "Found PCI device {info} at {device_function}, status {status:?} command {command:?}"
            );

            let Some(virtio_type) = virtio_device_type(&info) else {
                continue;
            };
            debug!("  VirtIO {virtio_type:?}");

            return self.pci_transport(device_function).ok();
        }
    }
}
