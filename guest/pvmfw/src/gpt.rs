// Copyright 2023, The Android Open Source Project
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

//! Support for parsing GUID partition tables.

use core::cmp::min;
use core::fmt;
use core::mem::size_of;
use core::ops::RangeInclusive;
use core::slice;
use static_assertions::const_assert;
use static_assertions::const_assert_eq;
use uuid::Uuid;
use virtio_drivers::device::blk::SECTOR_SIZE;
use vmbase::util::ceiling_div;
use vmbase::virtio::{pci, HalImpl};
use zerocopy::FromBytes;
use zerocopy::FromZeroes;

type VirtIOBlk = pci::VirtIOBlk<HalImpl>;

pub enum Error {
    /// VirtIO error during read operation.
    FailedRead(virtio_drivers::Error),
    /// VirtIO error during write operation.
    FailedWrite(virtio_drivers::Error),
    /// Invalid GPT header.
    InvalidHeader,
    /// Invalid partition block index.
    BlockOutsidePartition(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::FailedRead(e) => write!(f, "Failed to read from disk: {e}"),
            Self::FailedWrite(e) => write!(f, "Failed to write to disk: {e}"),
            Self::InvalidHeader => write!(f, "Found invalid GPT header"),
            Self::BlockOutsidePartition(i) => write!(f, "Accessed invalid block index {i}"),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

pub struct Partition {
    partitions: Partitions,
    indices: RangeInclusive<usize>,
}

impl Partition {
    pub fn get_by_name(device: VirtIOBlk, name: &str) -> Result<Option<Self>> {
        Partitions::new(device)?.get_partition_by_name(name)
    }

    fn new(partitions: Partitions, entry: &Entry) -> Self {
        let first = entry.first_lba().try_into().unwrap();
        let last = entry.last_lba().try_into().unwrap();

        Self { partitions, indices: first..=last }
    }

    pub fn indices(&self) -> RangeInclusive<usize> {
        self.indices.clone()
    }

    pub fn read_block(&mut self, index: usize, blk: &mut [u8]) -> Result<()> {
        let index = self.block_index(index).ok_or(Error::BlockOutsidePartition(index))?;
        self.partitions.read_block(index, blk)
    }

    pub fn write_block(&mut self, index: usize, blk: &[u8]) -> Result<()> {
        let index = self.block_index(index).ok_or(Error::BlockOutsidePartition(index))?;
        self.partitions.write_block(index, blk)
    }

    fn block_index(&self, index: usize) -> Option<usize> {
        if self.indices.contains(&index) {
            Some(index)
        } else {
            None
        }
    }
}

pub struct Partitions {
    device: VirtIOBlk,
    entries_count: usize,
}

impl Partitions {
    pub const LBA_SIZE: usize = SECTOR_SIZE;

    fn new(mut device: VirtIOBlk) -> Result<Self> {
        let mut blk = [0; Self::LBA_SIZE];
        device.read_blocks(Header::LBA, &mut blk).map_err(Error::FailedRead)?;
        let header = Header::read_from_prefix(blk.as_slice()).unwrap();
        if !header.is_valid() {
            return Err(Error::InvalidHeader);
        }
        let entries_count = usize::try_from(header.entries_count()).unwrap();

        Ok(Self { device, entries_count })
    }

    fn get_partition_by_name(mut self, name: &str) -> Result<Option<Partition>> {
        const_assert_eq!(Partitions::LBA_SIZE.rem_euclid(size_of::<Entry>()), 0);
        let entries_per_blk = Partitions::LBA_SIZE.checked_div(size_of::<Entry>()).unwrap();

        // Create a UTF-16 reference against which we'll compare partition names. Note that unlike
        // the C99 wcslen(), this comparison will cover bytes past the first L'\0' character.
        let mut needle = [0; Entry::NAME_SIZE / size_of::<u16>()];
        for (dest, src) in needle.iter_mut().zip(name.encode_utf16()) {
            *dest = src;
        }

        let mut blk = [0; Self::LBA_SIZE];
        let mut rem = self.entries_count;
        let num_blocks = ceiling_div(self.entries_count, entries_per_blk).unwrap();
        for i in Header::ENTRIES_LBA..Header::ENTRIES_LBA.checked_add(num_blocks).unwrap() {
            self.read_block(i, &mut blk)?;
            let entries = blk.as_ptr().cast::<Entry>();
            // SAFETY: blk is assumed to be properly aligned for Entry and its size is assert-ed
            // above. All potential values of the slice will produce valid Entry values.
            let entries = unsafe { slice::from_raw_parts(entries, min(rem, entries_per_blk)) };
            for entry in entries {
                let entry_name = entry.name;
                if entry_name == needle {
                    return Ok(Some(Partition::new(self, entry)));
                }
                rem -= 1;
            }
        }
        Ok(None)
    }

    fn read_block(&mut self, index: usize, blk: &mut [u8]) -> Result<()> {
        self.device.read_blocks(index, blk).map_err(Error::FailedRead)
    }

    fn write_block(&mut self, index: usize, blk: &[u8]) -> Result<()> {
        self.device.write_blocks(index, blk).map_err(Error::FailedWrite)
    }
}

type Lba = u64;

/// Structure as defined in release 2.10 of the UEFI Specification (5.3.2 GPT Header).
#[derive(FromZeroes, FromBytes)]
#[repr(C, packed)]
struct Header {
    signature: u64,
    revision: u32,
    header_size: u32,
    header_crc32: u32,
    reserved0: u32,
    current_lba: Lba,
    backup_lba: Lba,
    first_lba: Lba,
    last_lba: Lba,
    disk_guid: u128,
    entries_lba: Lba,
    entries_count: u32,
    entry_size: u32,
    entries_crc32: u32,
}
const_assert!(size_of::<Header>() < Partitions::LBA_SIZE);

impl Header {
    const SIGNATURE: u64 = u64::from_le_bytes(*b"EFI PART");
    const REVISION_1_0: u32 = 1 << 16;
    const LBA: usize = 1;
    const ENTRIES_LBA: usize = 2;

    fn is_valid(&self) -> bool {
        self.signature() == Self::SIGNATURE
            && self.header_size() == size_of::<Self>().try_into().unwrap()
            && self.revision() == Self::REVISION_1_0
            && self.entry_size() == size_of::<Entry>().try_into().unwrap()
            && self.current_lba() == Self::LBA.try_into().unwrap()
            && self.entries_lba() == Self::ENTRIES_LBA.try_into().unwrap()
    }

    fn signature(&self) -> u64 {
        u64::from_le(self.signature)
    }

    fn entries_count(&self) -> u32 {
        u32::from_le(self.entries_count)
    }

    fn header_size(&self) -> u32 {
        u32::from_le(self.header_size)
    }

    fn revision(&self) -> u32 {
        u32::from_le(self.revision)
    }

    fn entry_size(&self) -> u32 {
        u32::from_le(self.entry_size)
    }

    fn entries_lba(&self) -> Lba {
        Lba::from_le(self.entries_lba)
    }

    fn current_lba(&self) -> Lba {
        Lba::from_le(self.current_lba)
    }
}

/// Structure as defined in release 2.10 of the UEFI Specification (5.3.3 GPT Partition Entry
/// Array).
#[repr(C, packed)]
struct Entry {
    type_guid: Uuid,
    guid: Uuid,
    first_lba: Lba,
    last_lba: Lba,
    flags: u64,
    name: [u16; Entry::NAME_SIZE / size_of::<u16>()], // UTF-16
}

impl Entry {
    const NAME_SIZE: usize = 72;

    fn first_lba(&self) -> Lba {
        Lba::from_le(self.first_lba)
    }

    fn last_lba(&self) -> Lba {
        Lba::from_le(self.last_lba)
    }
}
