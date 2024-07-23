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

//! Support for the pvmfw configuration data format.

use core::fmt;
use core::mem;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::result;
use log::{info, warn};
use static_assertions::const_assert_eq;
use vmbase::util::RangeExt;
use zerocopy::{FromBytes, FromZeroes};

/// Configuration data header.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, FromZeroes, FromBytes)]
struct Header {
    /// Magic number; must be `Header::MAGIC`.
    magic: u32,
    /// Version of the header format.
    version: Version,
    /// Total size of the configuration data.
    total_size: u32,
    /// Feature flags; currently reserved and must be zero.
    flags: u32,
}

#[derive(Debug)]
pub enum Error {
    /// Reserved region can't fit configuration header.
    BufferTooSmall,
    /// Header has the wrong alignment
    HeaderMisaligned,
    /// Header doesn't contain the expect magic value.
    InvalidMagic,
    /// Version of the header isn't supported.
    UnsupportedVersion(Version),
    /// Header describes configuration data that doesn't fit in the expected buffer.
    InvalidSize(usize),
    /// Header entry is missing.
    MissingEntry(Entry),
    /// Range described by entry does not fit within config data.
    EntryOutOfBounds(Entry, Range<usize>, Range<usize>),
    /// Entries are in out of order
    EntryOutOfOrder,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::BufferTooSmall => write!(f, "Reserved region is smaller than config header"),
            Self::HeaderMisaligned => write!(f, "Reserved region is misaligned"),
            Self::InvalidMagic => write!(f, "Wrong magic number"),
            Self::UnsupportedVersion(v) => write!(f, "Version {v} not supported"),
            Self::InvalidSize(sz) => write!(f, "Total size ({sz:#x}) overflows reserved region"),
            Self::MissingEntry(entry) => write!(f, "Mandatory {entry:?} entry is missing"),
            Self::EntryOutOfBounds(entry, range, limits) => {
                write!(
                    f,
                    "Entry {entry:?} out of bounds: {range:#x?} must be within range {limits:#x?}"
                )
            }
            Self::EntryOutOfOrder => write!(f, "Entries are out of order"),
        }
    }
}

pub type Result<T> = result::Result<T, Error>;

impl Header {
    const MAGIC: u32 = u32::from_ne_bytes(*b"pvmf");
    const VERSION_1_0: Version = Version { major: 1, minor: 0 };
    const VERSION_1_1: Version = Version { major: 1, minor: 1 };
    const VERSION_1_2: Version = Version { major: 1, minor: 2 };

    pub fn total_size(&self) -> usize {
        self.total_size as usize
    }

    pub fn body_lowest_bound(&self) -> Result<usize> {
        let entries_offset = mem::size_of::<Self>();

        // Ensure that the entries are properly aligned and do not require padding.
        const_assert_eq!(mem::align_of::<Header>() % mem::align_of::<HeaderEntry>(), 0);
        const_assert_eq!(mem::size_of::<Header>() % mem::align_of::<HeaderEntry>(), 0);

        let entries_size = self.entry_count()?.checked_mul(mem::size_of::<HeaderEntry>()).unwrap();

        Ok(entries_offset.checked_add(entries_size).unwrap())
    }

    pub fn entry_count(&self) -> Result<usize> {
        let last_entry = match self.version {
            Self::VERSION_1_0 => Entry::DebugPolicy,
            Self::VERSION_1_1 => Entry::VmDtbo,
            Self::VERSION_1_2 => Entry::VmBaseDtbo,
            v @ Version { major: 1, .. } => {
                const LATEST: Version = Header::VERSION_1_2;
                warn!("Parsing unknown config data version {v} as version {LATEST}");
                return Ok(Entry::COUNT);
            }
            v => return Err(Error::UnsupportedVersion(v)),
        };

        Ok(last_entry as usize + 1)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Entry {
    Bcc,
    DebugPolicy,
    VmDtbo,
    VmBaseDtbo,
    #[allow(non_camel_case_types)] // TODO: Use mem::variant_count once stable.
    _VARIANT_COUNT,
}

impl Entry {
    const COUNT: usize = Self::_VARIANT_COUNT as usize;

    const ALL_ENTRIES: [Entry; Self::COUNT] =
        [Self::Bcc, Self::DebugPolicy, Self::VmDtbo, Self::VmBaseDtbo];
}

#[derive(Default)]
pub struct Entries<'a> {
    pub bcc: &'a mut [u8],
    pub debug_policy: Option<&'a [u8]>,
    pub vm_dtbo: Option<&'a mut [u8]>,
    pub vm_ref_dt: Option<&'a [u8]>,
}

#[repr(packed)]
#[derive(Clone, Copy, Debug, FromZeroes, FromBytes)]
struct HeaderEntry {
    offset: u32,
    size: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Eq, FromZeroes, FromBytes, PartialEq)]
pub struct Version {
    minor: u16,
    major: u16,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Copy the fields to local variables to prevent unaligned access.
        let (major, minor) = (self.major, self.minor);
        write!(f, "{}.{}", major, minor)
    }
}

/// Range with non-empty length.
#[derive(Debug, Copy, Clone)]
struct NonEmptyRange {
    start: usize,
    size: NonZeroUsize,
}

impl NonEmptyRange {
    pub fn new(start: usize, size: usize) -> Option<Self> {
        // Ensure end() is safe.
        start.checked_add(size).unwrap();

        Some(Self { start, size: NonZeroUsize::new(size)? })
    }

    fn end(&self) -> usize {
        self.start + self.len()
    }

    fn len(&self) -> usize {
        self.size.into()
    }

    fn as_range(&self) -> Range<usize> {
        self.start..self.end()
    }
}

#[derive(Debug)]
pub struct Config<'a> {
    body: &'a mut [u8],
    ranges: [Option<NonEmptyRange>; Entry::COUNT],
}

impl<'a> Config<'a> {
    /// Take ownership of a pvmfw configuration consisting of its header and following entries.
    pub fn new(bytes: &'a mut [u8]) -> Result<Self> {
        const HEADER_SIZE: usize = mem::size_of::<Header>();
        if bytes.len() < HEADER_SIZE {
            return Err(Error::BufferTooSmall);
        }

        let (header, rest) =
            zerocopy::Ref::<_, Header>::new_from_prefix(bytes).ok_or(Error::HeaderMisaligned)?;
        let header = header.into_ref();

        if header.magic != Header::MAGIC {
            return Err(Error::InvalidMagic);
        }

        let header_flags = header.flags;
        if header_flags != 0 {
            warn!("Ignoring unknown config flags: {header_flags:#x}");
        }

        info!("pvmfw config version: {}", header.version);

        // Validate that we won't get an invalid alignment in the following due to padding to u64.
        const_assert_eq!(HEADER_SIZE % mem::size_of::<u64>(), 0);

        // Ensure that Header::total_size isn't larger than anticipated by the caller and resize
        // the &[u8] to catch OOB accesses to entries/blobs.
        let total_size = header.total_size();
        let rest = if let Some(rest_size) = total_size.checked_sub(HEADER_SIZE) {
            rest.get_mut(..rest_size).ok_or(Error::InvalidSize(total_size))?
        } else {
            return Err(Error::InvalidSize(total_size));
        };

        let (header_entries, body) =
            zerocopy::Ref::<_, [HeaderEntry]>::new_slice_from_prefix(rest, header.entry_count()?)
                .ok_or(Error::BufferTooSmall)?;

        // Validate that we won't get an invalid alignment in the following due to padding to u64.
        const_assert_eq!(mem::size_of::<HeaderEntry>() % mem::size_of::<u64>(), 0);

        // Ensure entries are in the body.
        let limits = header.body_lowest_bound()?..total_size;
        let mut ranges: [Option<NonEmptyRange>; Entry::COUNT] = [None; Entry::COUNT];
        let mut last_end = 0;
        for entry in Entry::ALL_ENTRIES {
            let Some(header_entry) = header_entries.get(entry as usize) else { continue };
            let entry_offset = header_entry.offset.try_into().unwrap();
            let entry_size = header_entry.size.try_into().unwrap();
            let Some(range) = NonEmptyRange::new(entry_offset, entry_size) else { continue };
            let range = range.as_range();
            if !range.is_within(&limits) {
                return Err(Error::EntryOutOfBounds(entry, range, limits));
            }

            if last_end > range.start {
                return Err(Error::EntryOutOfOrder);
            }
            last_end = range.end;

            ranges[entry as usize] = NonEmptyRange::new(
                entry_offset - limits.start, // is_within() validates safety of this.
                entry_size,
            );
        }
        // Ensures that BCC exists.
        ranges[Entry::Bcc as usize].ok_or(Error::MissingEntry(Entry::Bcc))?;

        Ok(Self { body, ranges })
    }

    /// Locate the various config entries.
    pub fn get_entries(self) -> Entries<'a> {
        // We require the blobs to be in the same order as the `Entry` enum (and this is checked
        // in `new` above)
        // So we can just work through the body range and split off the parts we are interested in.
        let mut offset = 0;
        let mut body = self.body;

        let mut entries: [Option<&mut [u8]>; Entry::COUNT] = Default::default();
        for (i, range) in self.ranges.iter().enumerate() {
            if let Some(range) = range {
                body = &mut body[range.start - offset..];
                let (chunk, rest) = body.split_at_mut(range.len());
                offset = range.end();
                body = rest;
                entries[i] = Some(chunk);
            }
        }
        let [bcc, debug_policy, vm_dtbo, vm_ref_dt] = entries;

        // The platform BCC has always been required.
        let bcc = bcc.unwrap();

        // We have no reason to mutate so drop the `mut`.
        let debug_policy = debug_policy.map(|x| &*x);
        let vm_ref_dt = vm_ref_dt.map(|x| &*x);

        Entries { bcc, debug_policy, vm_dtbo, vm_ref_dt }
    }
}
