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

//! Linux x86 boot protocol and bzImage kernel format structures
//!
//! <https://docs.kernel.org/arch/x86/boot.html>

use core::mem;

use static_assertions::const_assert_eq;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// Linux setup data linked list entry
#[derive(Debug, FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned)]
#[repr(C, packed)]
pub struct setup_data_header {
    /// Address of next node in linked list
    pub next: u64,
    /// Type of node
    pub type_: u32,
    /// Data length in this node [bytes]
    pub len: u32,
}

const_assert_eq!(mem::size_of::<setup_data_header>(), 16);

impl setup_data_header {
    /// Device Tree Blob type
    pub const SETUP_DTB: u32 = 2;
}

/// Returns the data of first setup_data entry matching the type: type_.
///
/// we expect the setup_data entries to be packed contiguously one after the other
/// in the same order as they are in the linked list. crosvm packs it this way and
/// verifying this order implicitly verifies that the data does not overlap.
pub fn find_setup_data_entry(mut setup_data_slice: &mut [u8], type_: u32) -> Option<&mut [u8]> {
    let mut hdr;

    loop {
        (hdr, setup_data_slice) = setup_data_header::mut_from_prefix(setup_data_slice).ok()?;

        if hdr.type_ == type_ {
            return if hdr.len > 0 { setup_data_slice.get_mut(..hdr.len as usize) } else { None };
        }

        if hdr.next == 0 {
            return None;
        }

        let next = usize::try_from(hdr.next).unwrap();
        let offset = next.checked_sub(setup_data_slice.as_ptr() as _).unwrap();
        setup_data_slice = setup_data_slice.get_mut(offset..)?;
    }
}

/// Structure of "zero page" memory
#[derive(FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned)]
#[repr(C, packed)]
pub struct boot_params {
    _dontcare0: [u8; 488],

    /// Number of entries in `e820_table`.
    pub e820_entries: u8,

    _dontcare1: [u8; 8],

    /// Linux boot protocol header
    pub hdr: setup_header,

    _dontcare2: [u8; 100],

    /// Physical memory layout
    pub e820_table: [e820_entry; 128],

    _dontcare3: [u8; 816],
}

const_assert_eq!(mem::offset_of!(boot_params, e820_entries), 0x1e8);
const_assert_eq!(mem::offset_of!(boot_params, hdr), 0x1f1);
const_assert_eq!(mem::offset_of!(boot_params, e820_table), 0x2d0);
const_assert_eq!(mem::size_of::<boot_params>(), 0x1000);

impl boot_params {
    /// Add new e820 entry
    /// TODO(b/432207991): validation and sanitization
    pub fn push_e820_entry(&mut self, addr: u64, size: u64, type_: u32) {
        let next_entry = usize::from(self.e820_entries);
        let entry = self.e820_table.get_mut(next_entry).expect("out of e820 entries");
        entry.addr = addr;
        entry.size = size;
        entry.type_ = type_;
        self.e820_entries += 1;
    }
}

/// Linux x86 bzImage header
#[derive(Debug, FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned)]
#[repr(C, packed)]
pub struct setup_header {
    setup_sects: u8,
    root_flags: u16,
    syssize: u32,
    ram_size: u16,
    vid_mode: u16,
    root_dev: u16,
    boot_flag: u16,
    jump: [u8; 2],
    header: [u8; 4],
    version: u16,
    realmode_swtch: u32,
    start_sys_seg: u16,
    kernel_version: u16,
    type_of_loader: u8,
    loadflags: u8,
    setup_move_size: u16,
    code32_start: u32,
    ramdisk_image: u32,
    ramdisk_size: u32,
    bootsect_kludge: u32,
    heap_end_ptr: u16,
    ext_loader_ver: u8,
    ext_loader_type: u8,
    cmd_line_ptr: u32,
    initrd_addr_max: u32,
    kernel_alignment: u32,
    relocatable_kernel: u8,
    min_alignment: u8,
    xloadflags: u16,
    cmdline_size: u32,
    hardware_subarch: u32,
    hardware_subarch_data: u64,
    payload_offset: u32,
    payload_length: u32,
    setup_data: u64,
    pref_address: u64,
    init_size: u32,
    handover_offset: u32,
    kernel_info_offset: u32,
}

impl setup_header {
    /// Offset of `setup_header` from the beginning of a bzImage kernel.
    pub const OFFSET: usize = 0x01f1;

    /// Expected `boot_flag` magic number.
    pub const BOOT_FLAG: u16 = 0xAA55;

    /// Expected `header` magic number.
    pub const HEADER_MAGIC: [u8; 4] = *b"HdrS";

    /// 64-bit entry point offset from the beginning of the kernel code.
    pub const ENTRY_POINT_64_OFFSET: usize = 0x200;

    /// Number of sectors for the boot code preceding `setup_sects`.
    pub const BOOT_SECTS: usize = 1;

    /// Size of sector for `BOOT_SECTS` and `setup_sects`.
    pub const SECTOR_SIZE: usize = 512;

    /// Returns the size of the setup code in units of 512-byte sectors.
    pub fn setup_sects(&self) -> usize {
        if self.setup_sects == 0 {
            4
        } else {
            usize::from(self.setup_sects)
        }
    }

    /// Returns 32-bit protected mode entry point offset.
    pub fn entry_point_32_offset(&self) -> usize {
        // 32-bit entry point follows the boot sector and setup code.
        (Self::BOOT_SECTS + self.setup_sects()) * Self::SECTOR_SIZE
    }

    /// Returns 64-bit entry point offset.
    pub fn entry_point_64_offset(&self) -> usize {
        self.entry_point_32_offset() + Self::ENTRY_POINT_64_OFFSET
    }

    /// Returns setup_data linked list head pointer.
    pub fn setup_data(&self) -> usize {
        self.setup_data.try_into().unwrap()
    }

    /// Attempts to parse a bzImage kernel and return its header, if valid.
    pub fn get_from_bzimage(kernel: &[u8]) -> Option<&Self> {
        let hdr_bytes = kernel.get(Self::OFFSET..)?;
        let (hdr, _rest) = setup_header::ref_from_prefix(hdr_bytes).ok()?;

        if hdr.boot_flag != Self::BOOT_FLAG {
            return None;
        }

        if hdr.header != Self::HEADER_MAGIC {
            return None;
        }

        Some(hdr)
    }
}

/// boot_params::e820_table entry
#[derive(Debug, FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned)]
#[repr(C, packed)]
pub struct e820_entry {
    /// Memory region start address
    pub addr: u64,
    /// Memory region size
    pub size: u64,
    /// Memory region type
    pub type_: u32,
}

const_assert_eq!(mem::size_of::<e820_entry>(), 20);

impl e820_entry {
    /// Normal usable memory
    pub const TYPE_RAM: u32 = 1;
    /// Reserved memory
    pub const TYPE_RESERVED: u32 = 2;
    /// ACPI reclaimable memory
    pub const TYPE_ACPI: u32 = 3;
    /// ACPI NVS memory
    pub const TYPE_NVS: u32 = 4;
    /// Bad unusable memory
    pub const TYPE_UNUSABLE: u32 = 5;
}
