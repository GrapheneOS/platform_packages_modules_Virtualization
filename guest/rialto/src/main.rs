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

//! Project Rialto main source file.

#![no_main]
#![no_std]

mod communication;
mod error;
mod exceptions;
mod fdt;

extern crate alloc;

use crate::communication::VsockStream;
use crate::error::{Error, Result};
use crate::fdt::{read_dice_range_from, read_is_strict_boot, read_vendor_hashtree_root_digest};
use alloc::boxed::Box;
use ciborium_io::Write;
use core::num::NonZeroUsize;
use core::slice;
use diced_open_dice::{bcc_handover_parse, DiceArtifacts};
use fdtpci::PciInfo;
use libfdt::FdtError;
use log::{debug, error, info};
use service_vm_comm::{ServiceVmRequest, VmType};
use service_vm_fake_chain::service_vm;
use service_vm_requests::{process_request, RequestContext};
use virtio_drivers::{
    device::socket::{VsockAddr, VMADDR_CID_HOST},
    transport::{pci::bus::PciRoot, DeviceType, Transport},
    Hal,
};
use vmbase::{
    configure_heap,
    fdt::SwiotlbInfo,
    generate_image_header,
    hyp::{get_mem_sharer, get_mmio_guard},
    layout::{self, crosvm, UART_PAGE_ADDR},
    main,
    memory::{MemoryTracker, PageTable, MEMORY, PAGE_SIZE, SIZE_128KB},
    power::reboot,
    virtio::{
        pci::{self, PciTransportIterator, VirtIOSocket},
        HalImpl,
    },
};

fn host_addr(fdt: &libfdt::Fdt) -> Result<VsockAddr> {
    Ok(VsockAddr { cid: VMADDR_CID_HOST, port: vm_type(fdt)?.port() })
}

fn vm_type(fdt: &libfdt::Fdt) -> Result<VmType> {
    if read_is_strict_boot(fdt)? {
        Ok(VmType::ProtectedVm)
    } else {
        Ok(VmType::NonProtectedVm)
    }
}

fn new_page_table() -> Result<PageTable> {
    let mut page_table = PageTable::default();

    page_table.map_data(&layout::scratch_range().into())?;
    page_table.map_data(&layout::stack_range(40 * PAGE_SIZE).into())?;
    page_table.map_code(&layout::text_range().into())?;
    page_table.map_rodata(&layout::rodata_range().into())?;
    page_table.map_device(&layout::console_uart_page().into())?;

    Ok(page_table)
}

/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
/// * The `fdt_addr` must be a valid pointer and points to a valid `Fdt`.
unsafe fn try_main(fdt_addr: usize) -> Result<()> {
    info!("Welcome to Rialto!");
    let page_table = new_page_table()?;

    MEMORY.lock().replace(MemoryTracker::new(
        page_table,
        crosvm::MEM_START..layout::MAX_VIRT_ADDR,
        crosvm::MMIO_RANGE,
        None, // Rialto doesn't have any payload for now.
    ));

    let fdt_range = MEMORY
        .lock()
        .as_mut()
        .unwrap()
        .alloc(fdt_addr, NonZeroUsize::new(crosvm::FDT_MAX_SIZE).unwrap())?;
    // SAFETY: The tracker validated the range to be in main memory, mapped, and not overlap.
    let fdt = unsafe { slice::from_raw_parts(fdt_range.start as *mut u8, fdt_range.len()) };
    // We do not need to validate the DT since it is already validated in pvmfw.
    let fdt = libfdt::Fdt::from_slice(fdt)?;

    let memory_range = fdt.first_memory_range()?;
    MEMORY.lock().as_mut().unwrap().shrink(&memory_range).inspect_err(|_| {
        error!("Failed to use memory range value from DT: {memory_range:#x?}");
    })?;

    if let Some(mem_sharer) = get_mem_sharer() {
        let granule = mem_sharer.granule()?;
        MEMORY.lock().as_mut().unwrap().init_dynamic_shared_pool(granule).inspect_err(|_| {
            error!("Failed to initialize dynamically shared pool.");
        })?;
    } else if let Ok(swiotlb_info) = SwiotlbInfo::new_from_fdt(fdt) {
        let range = swiotlb_info.fixed_range().ok_or_else(|| {
            error!("Pre-shared pool range not specified in swiotlb node");
            Error::from(FdtError::BadValue)
        })?;
        MEMORY.lock().as_mut().unwrap().init_static_shared_pool(range).inspect_err(|_| {
            error!("Failed to initialize pre-shared pool.");
        })?;
    } else {
        info!("No MEM_SHARE capability detected or swiotlb found: allocating buffers from heap.");
        MEMORY.lock().as_mut().unwrap().init_heap_shared_pool().inspect_err(|_| {
            error!("Failed to initialize heap-based pseudo-shared pool.");
        })?;
    }

    let bcc_handover: Box<dyn DiceArtifacts> = match vm_type(fdt)? {
        VmType::ProtectedVm => {
            let dice_range = read_dice_range_from(fdt)?;
            info!("DICE range: {dice_range:#x?}");
            // SAFETY: This region was written by pvmfw in its writable_data region. The region
            // has no overlap with the main memory region and is safe to be mapped as read-only
            // data.
            let res = unsafe {
                MEMORY.lock().as_mut().unwrap().alloc_range_outside_main_memory(&dice_range)
            };
            res.inspect_err(|_| {
                error!("Failed to use DICE range from DT: {dice_range:#x?}");
            })?;
            let dice_start = dice_range.start as *const u8;
            // SAFETY: There's no memory overlap and the region is mapped as read-only data.
            let bcc_handover = unsafe { slice::from_raw_parts(dice_start, dice_range.len()) };
            Box::new(bcc_handover_parse(bcc_handover)?)
        }
        // Currently, a sample DICE data is used for non-protected VMs, as these VMs only run
        // in tests at the moment.
        VmType::NonProtectedVm => Box::new(service_vm::fake_service_vm_dice_artifacts()?),
    };

    let pci_info = PciInfo::from_fdt(fdt)?;
    debug!("PCI: {pci_info:#x?}");
    let mut pci_root = pci::initialize(pci_info, MEMORY.lock().as_mut().unwrap())
        .map_err(Error::PciInitializationFailed)?;
    debug!("PCI root: {pci_root:#x?}");
    let socket_device = find_socket_device::<HalImpl>(&mut pci_root)?;
    debug!("Found socket device: guest cid = {:?}", socket_device.guest_cid());
    let vendor_hashtree_root_digest = read_vendor_hashtree_root_digest(fdt)?;
    let request_context =
        RequestContext { dice_artifacts: bcc_handover.as_ref(), vendor_hashtree_root_digest };

    let mut vsock_stream = VsockStream::new(socket_device, host_addr(fdt)?)?;
    while let ServiceVmRequest::Process(req) = vsock_stream.read_request()? {
        info!("Received request: {}", req.name());
        let response = process_request(req, &request_context);
        info!("Sending response: {}", response.name());
        vsock_stream.write_response(&response)?;
        vsock_stream.flush()?;
    }
    vsock_stream.shutdown()?;

    Ok(())
}

fn find_socket_device<T: Hal>(pci_root: &mut PciRoot) -> Result<VirtIOSocket<T>> {
    PciTransportIterator::<T>::new(pci_root)
        .find(|t| DeviceType::Socket == t.device_type())
        .map(VirtIOSocket::<T>::new)
        .transpose()
        .map_err(Error::VirtIOSocketCreationFailed)?
        .ok_or(Error::MissingVirtIOSocketDevice)
}

fn try_unshare_all_memory() -> Result<()> {
    info!("Starting unsharing memory...");

    // No logging after unmapping UART.
    if let Some(mmio_guard) = get_mmio_guard() {
        mmio_guard.unmap(UART_PAGE_ADDR)?;
    }
    // Unshares all memory and deactivates page table.
    drop(MEMORY.lock().take());
    Ok(())
}

fn unshare_all_memory() {
    if let Err(e) = try_unshare_all_memory() {
        error!("Failed to unshare the memory: {e}");
    }
}

/// Entry point for Rialto.
pub fn main(fdt_addr: u64, _a1: u64, _a2: u64, _a3: u64) {
    log::set_max_level(log::LevelFilter::Debug);
    // SAFETY: `fdt_addr` is supposed to be a valid pointer and points to
    // a valid `Fdt`.
    match unsafe { try_main(fdt_addr as usize) } {
        Ok(()) => unshare_all_memory(),
        Err(e) => {
            error!("Rialto failed with {e}");
            unshare_all_memory();
            reboot()
        }
    }
}

/// Flushes data caches over the provided address range.
///
/// # Safety
///
/// The provided address and size must be to an address range that is valid for read and write
/// (typically on the stack, .bss, .data, or provided BCC) from a single allocation
/// (e.g. stack array).
#[no_mangle]
unsafe extern "C" fn DiceClearMemory(
    _ctx: *mut core::ffi::c_void,
    size: usize,
    addr: *mut core::ffi::c_void,
) {
    use core::slice;
    use vmbase::memory::flushed_zeroize;

    // SAFETY: We require our caller to provide a valid range within a single object. The open-dice
    // always calls this on individual stack-allocated arrays which ensures that.
    let region = unsafe { slice::from_raw_parts_mut(addr as *mut u8, size) };
    flushed_zeroize(region)
}

generate_image_header!();
main!(main);
configure_heap!(SIZE_128KB * 2);
