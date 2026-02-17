// Copyright 2026, The Android Open Source Project
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

use alloc::vec::Vec;
use zerocopy::byteorder::U16;
use zerocopy::byteorder::U32;
use zerocopy::byteorder::U64;
use zerocopy::{FromBytes, Immutable, KnownLayout, NativeEndian};

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Failed indexing data at {0} for {1} bytes")]
    BadRange(usize, usize),
    #[error("Failed parsing header count")]
    FailedParsingEntryCount,
    #[error("Failed parsing header")]
    FailedParsingHeader,
    #[error("Invalid non-zero value for reserved field {0}: {1}")]
    NonZeroReservedField(usize, u64),
}

/// Safe abstraction of the pvmfw configuration data "blob" for extra trusted keys.
#[derive(Debug)]
pub struct TrustedKeysConfig<'a> {
    keys: Vec<&'a [u8]>,
}

impl<'a> TrustedKeysConfig<'a> {
    pub fn try_from_bytes(bytes: &'a [u8]) -> Result<Self, Error> {
        let (count, _) =
            u32::read_from_prefix(bytes).map_err(|_| Error::FailedParsingEntryCount)?;
        let count = usize::try_from(count).unwrap();
        let (header, data) = TrustedKeysHeader::ref_from_prefix_with_elems(bytes, count)
            .map_err(|_| Error::FailedParsingHeader)?;

        let mut keys = Vec::new();
        for entry in &header.entries {
            let base = entry.key_offset.get().into();
            let size = entry.key_size.get().into();
            let key = data.get(base..(base + size)).ok_or(Error::BadRange(base, size))?;
            let reserved_fields = [
                entry._reserved0.get().into(),
                entry._reserved1.get(),
                entry._reserved2.get(),
                entry._reserved3.get(),
            ];
            for (i, &n) in reserved_fields.iter().enumerate() {
                if n != 0 {
                    return Err(Error::NonZeroReservedField(i, n));
                }
            }
            keys.push(key)
        }

        Ok(Self { keys })
    }

    pub fn trusted_keys(&self) -> &[&'a [u8]] {
        &self.keys
    }
}

#[derive(Debug, FromBytes, KnownLayout, Immutable)]
#[repr(C)]
struct TrustedKeysHeader {
    entries_count: U32<NativeEndian>,
    entries: [TrustedKeysHeaderEntry],
}

#[derive(Debug, FromBytes, KnownLayout, Immutable)]
#[repr(C)]
struct TrustedKeysHeaderEntry {
    /// Size in bytes of the key.
    key_size: U16<NativeEndian>,
    /// Offset of the key in the data block: offset 0 is right after the header.
    key_offset: U16<NativeEndian>,
    /// Must be zero.
    _reserved0: U32<NativeEndian>,
    /// Must be zero.
    _reserved1: U64<NativeEndian>,
    /// Must be zero.
    _reserved2: U64<NativeEndian>,
    /// Must be zero.
    _reserved3: U64<NativeEndian>,
}
