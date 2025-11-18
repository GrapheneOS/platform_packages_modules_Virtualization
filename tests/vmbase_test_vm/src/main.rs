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

//! Main kernel source file of the VM used in testing of the low-level functionality.
//! For more information see ../README.md.

#![no_main]
#![no_std]

mod communication;
mod error;

use crate::communication::VsockStream;
use crate::error::{Error, Result};
use ciborium_io::Write;
use core::num::NonZeroUsize;
use core::slice;
use log::{error, info};
use virtio_drivers::device::socket::{VsockAddr, VMADDR_CID_HOST};
use virtio_drivers::transport::pci::bus::{ConfigurationAccess, PciRoot};
use virtio_drivers::transport::{DeviceType, Transport};
use virtio_drivers::Hal;
use vmbase::fdt::pci::initialize_from_fdt;
use vmbase::layout::crosvm;
use vmbase::memory::{map_rodata, resize_available_memory, SIZE_128KB};
use vmbase::power::reboot;
use vmbase::virtio::pci::{PciTransportIterator, VirtIOSocket};
use vmbase::virtio::HalImpl;
use vmbase::{configure_heap, generate_image_header, main};
use vmbase_test_vm_messages::{Request, Response, VM_PORT};

fn process_request(req: Request) -> Response {
    match req {
        Request::Reverse(v) => Response::Reverse(v.into_iter().rev().collect()),
        Request::Shutdown => unreachable!(),
    }
}

/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
/// * The `fdt_addr` must be a valid pointer and points to a valid `Fdt`.
unsafe fn try_main(fdt_addr: usize) -> Result<()> {
    info!("Welcome to test VM!");

    let fdt_size = NonZeroUsize::new(crosvm::FDT_MAX_SIZE).unwrap();
    map_rodata(fdt_addr, fdt_size)?;
    // SAFETY: The tracker validated the range to be in main memory, mapped, and not overlap.
    let fdt = unsafe { slice::from_raw_parts(fdt_addr as *mut u8, fdt_size.into()) };
    // We do not need to validate the DT since it is already validated in pvmfw.
    let fdt = libfdt::Fdt::from_slice(fdt)?;

    #[allow(unused_mut)]
    let mut memory_range = fdt.first_memory_range()?;
    // "/memory" may include the pvmfw region, which we don't supported reusing in rialto, so
    // truncate it off if present.
    #[cfg(target_arch = "aarch64")]
    if memory_range.start == crosvm::PVMFW_START {
        memory_range.start = crosvm::MEM_START;
    }
    resize_available_memory(&memory_range).inspect_err(|_| {
        error!("Failed to use memory range value from DT: {memory_range:#x?}");
    })?;

    info!("main memory region: {memory_range:#?}");

    let mut pci_root = initialize_from_fdt(fdt).map_err(Error::PciInitializationFailed)?;
    let socket_device = find_socket_device::<HalImpl>(&mut pci_root)?;
    info!("Found socket device: guest cid = {:?}", socket_device.guest_cid());
    let host_addr = VsockAddr { cid: VMADDR_CID_HOST, port: VM_PORT };
    let mut vsock_stream = VsockStream::new(socket_device, host_addr)?;
    info!("listening for messages from host");
    loop {
        let req = vsock_stream.read_request()?;
        info!("Received request: {req:?}");
        if req == Request::Shutdown {
            info!("Shutting down. Bye!");
            break;
        }
        let resp = process_request(req);
        info!("Sending response: {resp:?}");
        vsock_stream.write_response(&resp)?;
        vsock_stream.flush()?;
    }
    vsock_stream.shutdown()?;

    Ok(())
}

fn find_socket_device<T: Hal>(
    pci_root: &mut PciRoot<impl ConfigurationAccess>,
) -> Result<VirtIOSocket<T>> {
    PciTransportIterator::<T, _>::new(pci_root)
        .find(|t| DeviceType::Socket == t.device_type())
        .map(VirtIOSocket::<T>::new)
        .transpose()
        .map_err(Error::VirtIOSocketCreationFailed)?
        .ok_or(Error::MissingVirtIOSocketDevice)
}

/// Entry point for this VM.
pub fn main(argv: &[usize]) {
    log::set_max_level(log::LevelFilter::Debug);
    // SAFETY: pvmfw passes a valid pointer to a valid `Fdt` to the guest kernel entry point.
    if let Err(e) = unsafe { try_main(argv[0]) } {
        error!("test vm failed: {e:?}");
        reboot()
    }
}

generate_image_header!();
main!(main);
configure_heap!(SIZE_128KB * 2);
