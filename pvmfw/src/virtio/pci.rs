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

use super::hal::HalImpl;
use crate::{entry::RebootReason, memory::MemoryTracker};
use alloc::boxed::Box;
use fdtpci::{PciError, PciInfo};
use log::{debug, error, info};
use once_cell::race::OnceBox;
use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::{
        pci::{bus::PciRoot, virtio_device_type, PciTransport},
        DeviceType, Transport,
    },
};

pub(super) static PCI_INFO: OnceBox<PciInfo> = OnceBox::new();

/// Prepares to use VirtIO PCI devices.
///
/// In particular:
///
/// 1. Maps the PCI CAM and BAR range in the page table and MMIO guard.
/// 2. Stores the `PciInfo` for the VirtIO HAL to use later.
/// 3. Creates and returns a `PciRoot`.
///
/// This must only be called once; it will panic if it is called a second time.
pub fn initialise(pci_info: PciInfo, memory: &mut MemoryTracker) -> Result<PciRoot, RebootReason> {
    map_mmio(&pci_info, memory)?;

    PCI_INFO.set(Box::new(pci_info.clone())).expect("Tried to set PCI_INFO a second time");

    // Safety: This is the only place where we call make_pci_root, and `PCI_INFO.set` above will
    // panic if it is called a second time.
    Ok(unsafe { pci_info.make_pci_root() })
}

/// Maps the CAM and BAR range in the page table and MMIO guard.
fn map_mmio(pci_info: &PciInfo, memory: &mut MemoryTracker) -> Result<(), RebootReason> {
    memory.map_mmio_range(pci_info.cam_range.clone()).map_err(|e| {
        error!("Failed to map PCI CAM: {}", e);
        RebootReason::InternalError
    })?;

    memory
        .map_mmio_range(pci_info.bar_range.start as usize..pci_info.bar_range.end as usize)
        .map_err(|e| {
            error!("Failed to map PCI MMIO range: {}", e);
            RebootReason::InternalError
        })?;

    Ok(())
}

/// Finds VirtIO PCI devices.
pub fn find_virtio_devices(pci_root: &mut PciRoot) -> Result<(), PciError> {
    for (device_function, info) in pci_root.enumerate_bus(0) {
        let (status, command) = pci_root.get_status_command(device_function);
        debug!(
            "Found PCI device {} at {}, status {:?} command {:?}",
            info, device_function, status, command
        );
        if let Some(virtio_type) = virtio_device_type(&info) {
            debug!("  VirtIO {:?}", virtio_type);
            let mut transport = PciTransport::new::<HalImpl>(pci_root, device_function).unwrap();
            info!(
                "Detected virtio PCI device with device type {:?}, features {:#018x}",
                transport.device_type(),
                transport.read_device_features(),
            );
            if virtio_type == DeviceType::Block {
                let mut blk =
                    VirtIOBlk::<HalImpl, _>::new(transport).expect("failed to create blk driver");
                info!("Found {} KiB block device.", blk.capacity() * 512 / 1024);
                let mut data = [0; 512];
                blk.read_block(0, &mut data).expect("Failed to read block device");
            }
        }
    }

    Ok(())
}