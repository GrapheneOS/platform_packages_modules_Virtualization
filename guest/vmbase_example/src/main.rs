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

//! VM bootloader example.

#![no_main]
#![no_std]

mod exceptions;
mod layout;
mod pci;

extern crate alloc;

use crate::layout::{boot_stack_range, print_addresses, DEVICE_REGION};
use crate::pci::{check_pci, get_bar_region};
use aarch64_paging::paging::VirtualAddress;
use aarch64_paging::MapError;
use alloc::{vec, vec::Vec};
use core::mem;
use core::ptr::addr_of_mut;
use cstr::cstr;
use fdtpci::PciInfo;
use libfdt::Fdt;
use log::{debug, error, info, trace, warn, LevelFilter};
use vmbase::{
    bionic, configure_heap, generate_image_header,
    layout::{crosvm::FDT_MAX_SIZE, rodata_range, scratch_range, text_range},
    linker, logger, main,
    memory::{PageTable, SIZE_64KB},
    util::RangeExt as _,
};

static INITIALISED_DATA: [u32; 4] = [1, 2, 3, 4];
static mut ZEROED_DATA: [u32; 10] = [0; 10];
static mut MUTABLE_DATA: [u32; 4] = [1, 2, 3, 4];

generate_image_header!();
main!(main);
configure_heap!(SIZE_64KB);

fn init_page_table(page_table: &mut PageTable) -> Result<(), MapError> {
    page_table.map_device(&DEVICE_REGION)?;
    page_table.map_code(&text_range().into())?;
    page_table.map_rodata(&rodata_range().into())?;
    page_table.map_data(&scratch_range().into())?;
    page_table.map_data(&boot_stack_range().into())?;

    info!("Activating IdMap...");
    // SAFETY: page_table duplicates the static mappings for everything that the Rust code is
    // aware of so activating it shouldn't have any visible effect.
    unsafe {
        page_table.activate();
    }
    info!("Activated.");

    Ok(())
}

/// Entry point for VM bootloader.
pub fn main(arg0: u64, arg1: u64, arg2: u64, arg3: u64) {
    log::set_max_level(LevelFilter::Debug);

    info!("Hello world");
    info!("x0={:#018x}, x1={:#018x}, x2={:#018x}, x3={:#018x}", arg0, arg1, arg2, arg3);
    print_addresses();
    check_data();
    check_stack_guard();

    let mut page_table = PageTable::default();
    init_page_table(&mut page_table).unwrap();

    info!("Checking FDT...");
    let fdt_addr = usize::try_from(arg0).unwrap();
    // SAFETY: The DTB range is valid, writable memory, and we don't construct any aliases to it.
    let fdt = unsafe { core::slice::from_raw_parts_mut(fdt_addr as *mut u8, FDT_MAX_SIZE) };
    let fdt_region = (VirtualAddress(fdt_addr)..VirtualAddress(fdt_addr + fdt.len())).into();
    page_table.map_data(&fdt_region).unwrap();
    let fdt = Fdt::from_mut_slice(fdt).unwrap();
    info!("FDT passed verification.");
    check_fdt(fdt);

    let pci_info = PciInfo::from_fdt(fdt).unwrap();
    debug!("Found PCI CAM at {:#x}-{:#x}", pci_info.cam_range.start, pci_info.cam_range.end);

    modify_fdt(fdt);

    check_alloc();

    let bar_region = get_bar_region(&pci_info);
    if bar_region.is_within(&DEVICE_REGION) {
        // Avoid a MapError::BreakBeforeMakeViolation.
        info!("BAR region is within already mapped device region: skipping page table ops.");
    } else {
        page_table.map_device(&bar_region).unwrap();
    }

    check_data();
    check_dice();

    // SAFETY: This is the only place where `make_pci_root` is called.
    let mut pci_root = unsafe { pci_info.make_pci_root() };
    check_pci(&mut pci_root);

    emit_suppressed_log();

    info!("De-activating IdMap...");
    mem::drop(page_table); // Release PageTable and switch back to idmap.S
    info!("De-activated.");
}

fn check_stack_guard() {
    info!("Testing stack guard");
    // SAFETY: No concurrency issue should occur when running these tests.
    let stack_guard = unsafe { bionic::TLS.stack_guard };
    assert_ne!(stack_guard, 0);
    // Check that a NULL-terminating value is added for C functions consuming strings from stack.
    assert_eq!(stack_guard.to_ne_bytes().last(), Some(&0));
    // Check that the TLS and guard are properly accessible from the dedicated register.
    assert_eq!(stack_guard, bionic::__get_tls().stack_guard);
    // Check that the LLVM __stack_chk_guard alias is also properly set up.
    assert_eq!(
        stack_guard,
        // SAFETY: No concurrency issue should occur when running these tests.
        unsafe { linker::__stack_chk_guard },
    );
}

fn check_data() {
    info!("INITIALISED_DATA: {:?}", INITIALISED_DATA.as_ptr());
    // SAFETY: We only print the addresses of the static mutable variable, not actually access it.
    info!("ZEROED_DATA: {:?}", unsafe { ZEROED_DATA.as_ptr() });
    // SAFETY: We only print the addresses of the static mutable variable, not actually access it.
    info!("MUTABLE_DATA: {:?}", unsafe { MUTABLE_DATA.as_ptr() });

    assert_eq!(INITIALISED_DATA[0], 1);
    assert_eq!(INITIALISED_DATA[1], 2);
    assert_eq!(INITIALISED_DATA[2], 3);
    assert_eq!(INITIALISED_DATA[3], 4);

    // SAFETY: Nowhere else in the program accesses this static mutable variable, so there is no
    // chance of concurrent access.
    let zeroed_data = unsafe { &mut *addr_of_mut!(ZEROED_DATA) };
    // SAFETY: Nowhere else in the program accesses this static mutable variable, so there is no
    // chance of concurrent access.
    let mutable_data = unsafe { &mut *addr_of_mut!(MUTABLE_DATA) };

    for element in zeroed_data.iter() {
        assert_eq!(*element, 0);
    }

    zeroed_data[0] = 13;
    assert_eq!(zeroed_data[0], 13);
    zeroed_data[0] = 0;
    assert_eq!(zeroed_data[0], 0);

    assert_eq!(mutable_data[0], 1);
    assert_eq!(mutable_data[1], 2);
    assert_eq!(mutable_data[2], 3);
    assert_eq!(mutable_data[3], 4);
    mutable_data[0] += 41;
    assert_eq!(mutable_data[0], 42);
    mutable_data[0] -= 41;
    assert_eq!(mutable_data[0], 1);

    info!("Data looks good");
}

fn check_fdt(reader: &Fdt) {
    for reg in reader.memory().unwrap() {
        info!("memory @ {reg:#x?}");
    }

    let compatible = cstr!("ns16550a");

    for c in reader.compatible_nodes(compatible).unwrap() {
        let reg = c.reg().unwrap().unwrap().next().unwrap();
        info!("node compatible with '{}' at {reg:?}", compatible.to_str().unwrap());
    }
}

fn modify_fdt(writer: &mut Fdt) {
    writer.unpack().unwrap();
    info!("FDT successfully unpacked.");

    let path = cstr!("/memory");
    let node = writer.node_mut(path).unwrap().unwrap();
    let name = cstr!("child");
    let mut child = node.add_subnode(name).unwrap();
    info!("Created subnode '{}/{}'.", path.to_str().unwrap(), name.to_str().unwrap());

    let name = cstr!("str-property");
    child.appendprop(name, b"property-value\0").unwrap();
    info!("Appended property '{}'.", name.to_str().unwrap());

    let name = cstr!("pair-property");
    let addr = 0x0123_4567u64;
    let size = 0x89ab_cdefu64;
    child.appendprop_addrrange(name, addr, size).unwrap();
    info!("Appended property '{}'.", name.to_str().unwrap());

    let writer = child.fdt();
    writer.pack().unwrap();
    info!("FDT successfully packed.");

    info!("FDT checks done.");
}

fn check_alloc() {
    info!("Allocating a Vec...");
    let mut vector: Vec<u32> = vec![1, 2, 3, 4];
    assert_eq!(vector[0], 1);
    assert_eq!(vector[1], 2);
    assert_eq!(vector[2], 3);
    assert_eq!(vector[3], 4);
    vector[2] = 42;
    assert_eq!(vector[2], 42);
    info!("Vec seems to work.");
}

fn check_dice() {
    info!("Testing DICE integration...");
    let hash = diced_open_dice::hash("hello world".as_bytes()).expect("DiceHash failed");
    assert_eq!(
        hash,
        [
            0x30, 0x9e, 0xcc, 0x48, 0x9c, 0x12, 0xd6, 0xeb, 0x4c, 0xc4, 0x0f, 0x50, 0xc9, 0x02,
            0xf2, 0xb4, 0xd0, 0xed, 0x77, 0xee, 0x51, 0x1a, 0x7c, 0x7a, 0x9b, 0xcd, 0x3c, 0xa8,
            0x6d, 0x4c, 0xd8, 0x6f, 0x98, 0x9d, 0xd3, 0x5b, 0xc5, 0xff, 0x49, 0x96, 0x70, 0xda,
            0x34, 0x25, 0x5b, 0x45, 0xb0, 0xcf, 0xd8, 0x30, 0xe8, 0x1f, 0x60, 0x5d, 0xcf, 0x7d,
            0xc5, 0x54, 0x2e, 0x93, 0xae, 0x9c, 0xd7, 0x6f
        ]
    );
}

macro_rules! log_all_levels {
    ($msg:literal) => {{
        error!($msg);
        warn!($msg);
        info!($msg);
        debug!($msg);
        trace!($msg);
    }};
}

fn emit_suppressed_log() {
    {
        let _guard = logger::suppress();
        log_all_levels!("Suppressed message");
    }
    log_all_levels!("Unsuppressed message");
}
