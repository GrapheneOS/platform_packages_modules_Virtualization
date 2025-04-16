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

use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

/// Linux x86 bzImage header
#[derive(Debug, FromBytes, Immutable, KnownLayout, Unaligned)]
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
