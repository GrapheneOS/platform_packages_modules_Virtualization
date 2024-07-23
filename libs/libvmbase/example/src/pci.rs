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

//! Functions to scan the PCI bus for VirtIO device.

use aarch64_paging::paging::MemoryRegion;
use alloc::alloc::{alloc_zeroed, dealloc, handle_alloc_error, Layout};
use core::{mem::size_of, ptr::NonNull};
use fdtpci::PciInfo;
use log::{debug, info};
use virtio_drivers::{
    device::console::VirtIOConsole,
    transport::{
        pci::{bus::PciRoot, PciTransport},
        DeviceType, Transport,
    },
    BufferDirection, Error, Hal, PhysAddr, PAGE_SIZE,
};
use vmbase::virtio::pci::{self, PciTransportIterator};

/// The standard sector size of a VirtIO block device, in bytes.
const SECTOR_SIZE_BYTES: usize = 512;

/// The size in sectors of the test block device we expect.
const EXPECTED_SECTOR_COUNT: usize = 4;

pub fn check_pci(pci_root: &mut PciRoot) {
    let mut checked_virtio_device_count = 0;
    let mut block_device_count = 0;
    let mut socket_device_count = 0;
    for mut transport in PciTransportIterator::<HalImpl>::new(pci_root) {
        info!(
            "Detected virtio PCI device with device type {:?}, features {:#018x}",
            transport.device_type(),
            transport.read_device_features(),
        );
        match transport.device_type() {
            DeviceType::Block => {
                check_virtio_block_device(transport, block_device_count);
                block_device_count += 1;
                checked_virtio_device_count += 1;
            }
            DeviceType::Console => {
                check_virtio_console_device(transport);
                checked_virtio_device_count += 1;
            }
            DeviceType::Socket => {
                check_virtio_socket_device(transport);
                socket_device_count += 1;
                checked_virtio_device_count += 1;
            }
            _ => {}
        }
    }

    assert_eq!(checked_virtio_device_count, 6);
    assert_eq!(block_device_count, 2);
    assert_eq!(socket_device_count, 1);
}

/// Checks the given VirtIO block device.
fn check_virtio_block_device(transport: PciTransport, index: usize) {
    let mut blk = pci::VirtIOBlk::<HalImpl>::new(transport).expect("failed to create blk driver");
    info!("Found {} KiB block device.", blk.capacity() * SECTOR_SIZE_BYTES as u64 / 1024);
    match index {
        0 => {
            assert_eq!(blk.capacity(), EXPECTED_SECTOR_COUNT as u64);
            let mut data = [0; SECTOR_SIZE_BYTES * EXPECTED_SECTOR_COUNT];
            for i in 0..EXPECTED_SECTOR_COUNT {
                blk.read_blocks(i, &mut data[i * SECTOR_SIZE_BYTES..(i + 1) * SECTOR_SIZE_BYTES])
                    .expect("Failed to read block device.");
            }
            for (i, chunk) in data.chunks(size_of::<u32>()).enumerate() {
                assert_eq!(chunk, &(i as u32).to_le_bytes());
            }
            info!("Read expected data from block device.");
        }
        1 => {
            assert_eq!(blk.capacity(), 0);
            let mut data = [0; SECTOR_SIZE_BYTES];
            assert_eq!(blk.read_blocks(0, &mut data), Err(Error::IoError));
        }
        _ => panic!("Unexpected VirtIO block device index {}.", index),
    }
}

/// Checks the given VirtIO socket device.
fn check_virtio_socket_device(transport: PciTransport) {
    let socket = pci::VirtIOSocket::<HalImpl>::new(transport)
        .expect("Failed to create VirtIO socket driver");
    info!("Found socket device: guest_cid={}", socket.guest_cid());
}

/// Checks the given VirtIO console device.
fn check_virtio_console_device(transport: PciTransport) {
    let mut console = VirtIOConsole::<HalImpl, PciTransport>::new(transport)
        .expect("Failed to create VirtIO console driver");
    info!("Found console device: {:?}", console.info());
    for &c in b"Hello VirtIO console\n" {
        console.send(c).expect("Failed to send character to VirtIO console device");
    }
    info!("Wrote to VirtIO console.");
}

/// Gets the memory region in which BARs are allocated.
pub fn get_bar_region(pci_info: &PciInfo) -> MemoryRegion {
    MemoryRegion::new(pci_info.bar_range.start as usize, pci_info.bar_range.end as usize)
}

struct HalImpl;

/// SAFETY: See the 'Implementation Safety' comments on methods below for how they fulfill the
/// safety requirements of the unsafe `Hal` trait.
unsafe impl Hal for HalImpl {
    /// # Implementation Safety
    ///
    /// `dma_alloc` ensures the returned DMA buffer is not aliased with any other allocation or
    /// reference in the program until it is deallocated by `dma_dealloc` by allocating a unique
    /// block of memory using `alloc_zeroed`, which is guaranteed to allocate valid, unique and
    /// zeroed memory. We request an alignment of at least `PAGE_SIZE` from `alloc_zeroed`.
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        debug!("dma_alloc: pages={}", pages);
        let layout =
            Layout::from_size_align(pages.checked_mul(PAGE_SIZE).unwrap(), PAGE_SIZE).unwrap();
        assert_ne!(layout.size(), 0);
        // SAFETY: We just checked that the layout has a non-zero size.
        let vaddr = unsafe { alloc_zeroed(layout) };
        let vaddr =
            if let Some(vaddr) = NonNull::new(vaddr) { vaddr } else { handle_alloc_error(layout) };
        let paddr = virt_to_phys(vaddr);
        (paddr, vaddr)
    }

    unsafe fn dma_dealloc(paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        debug!("dma_dealloc: paddr={:#x}, pages={}", paddr, pages);
        let layout = Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap();
        // SAFETY: The memory was allocated by `dma_alloc` above using the same allocator, and the
        // layout is the same as was used then.
        unsafe {
            dealloc(vaddr.as_ptr(), layout);
        }
        0
    }

    /// # Implementation Safety
    ///
    /// The returned pointer must be valid because the `paddr` describes a valid MMIO region, and we
    /// previously mapped the entire PCI MMIO range. It can't alias any other allocations because
    /// the PCI MMIO range doesn't overlap with any other memory ranges.
    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as _).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        let vaddr = buffer.cast();
        // Nothing to do, as the host already has access to all memory.
        virt_to_phys(vaddr)
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // Nothing to do, as the host already has access to all memory and we didn't copy the buffer
        // anywhere else.
    }
}

fn virt_to_phys(vaddr: NonNull<u8>) -> PhysAddr {
    vaddr.as_ptr() as _
}
