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
use core::ffi::CStr;
use zerocopy::byteorder::U16;
use zerocopy::byteorder::U32;
use zerocopy::byteorder::U64;
use zerocopy::{FromBytes, Immutable, KnownLayout, NativeEndian};

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Failed parsing string at offset: {0}")]
    BadString(usize),
    #[error("Failed indexing data at offset: {0}")]
    BadOffset(usize),
    #[error("Failed parsing header count")]
    FailedParsingEntryCount,
    #[error("Failed parsing header")]
    FailedParsingHeader,
    #[error("Invalid non-zero value for reserved field {0}: {1}")]
    NonZeroReservedField(usize, u64),
}

/// Safe abstraction of the pvmfw configuration data "blob" for rollback protection.
#[derive(Debug)]
pub struct RollbackConfig<'a> {
    entries: Vec<RollbackConfigEntry<'a>>,
}

impl<'a> RollbackConfig<'a> {
    pub fn try_from_bytes(bytes: &'a [u8]) -> Result<Self, Error> {
        let (count, _) =
            u32::read_from_prefix(bytes).map_err(|_| Error::FailedParsingEntryCount)?;
        let count = usize::try_from(count).unwrap();
        let (header, data) = RollbackConfigHeader::ref_from_prefix_with_elems(bytes, count)
            .map_err(|_| Error::FailedParsingHeader)?;

        let mut entries = Vec::new();
        for entry in &header.entries {
            entries.push(RollbackConfigEntry::new(entry, data)?);
        }

        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[RollbackConfigEntry] {
        &self.entries
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RollbackConfigEntry<'a> {
    pub vm_name: &'a str,
    pub rollback_policy: RollbackConfigPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RollbackConfigPolicy {
    /// Prevent the VM payload from running.
    Reserved,
    /// Check minimum rollback index value, with fixed signing public key.
    MinimumRollbackIndex(u64, RollbackConfigKeyId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RollbackConfigKeyId {
    /// Payload must be signed with pvmfw's embedded public key.
    EmbeddedPublicKey,
    /// Payload must be signed with the `n`-th extra public key.
    ExtraTrustedKey { n: usize },
}

impl From<u16> for RollbackConfigKeyId {
    fn from(v: u16) -> Self {
        match v {
            u16::MAX => Self::EmbeddedPublicKey,
            n => Self::ExtraTrustedKey { n: n.into() },
        }
    }
}

impl<'a> RollbackConfigEntry<'a> {
    fn new(entry: &RollbackConfigHeaderEntry, data: &'a [u8]) -> Result<Self, Error> {
        let vm_name_offset = usize::try_from(entry.vm_name_offset.get()).unwrap();
        let vm_name_and_beyond =
            data.get(vm_name_offset..).ok_or(Error::BadOffset(vm_name_offset))?;
        let vm_name = CStr::from_bytes_until_nul(vm_name_and_beyond)
            .map_err(|_| Error::BadString(vm_name_offset))?
            .to_str()
            .map_err(|_| Error::BadString(vm_name_offset))?;

        let key_id = entry.key_id.get().into();
        let rollback_policy = match entry.rollback_index.get() {
            u64::MAX => RollbackConfigPolicy::Reserved,
            i => RollbackConfigPolicy::MinimumRollbackIndex(i, key_id),
        };

        let reserved_fields =
            [entry._reserved0.get().into(), entry._reserved1.get(), entry._reserved2.get()];
        for (i, &n) in reserved_fields.iter().enumerate() {
            if n != 0 {
                return Err(Error::NonZeroReservedField(i, n));
            }
        }

        Ok(Self { vm_name, rollback_policy })
    }
}

#[derive(Debug, FromBytes, KnownLayout, Immutable)]
#[repr(C)]
struct RollbackConfigHeader {
    entries_count: U32<NativeEndian>,
    entries: [RollbackConfigHeaderEntry],
}

#[derive(Debug, FromBytes, KnownLayout, Immutable)]
#[repr(C)]
struct RollbackConfigHeaderEntry {
    /// Minimum rollback index permitted for the image or `u64::MAX` if the VM is forbidden.
    /// See `AvbVBMetaImageHeader::rollback_index` in libavb for more on its use.
    rollback_index: U64<NativeEndian>,
    /// Offset of a null-terminated string in the data block: offset 0 is right after the header.
    vm_name_offset: U32<NativeEndian>,
    /// Index of the config data trusted public key or `u16::MAX` for the embedded `PUBLIC_KEY`.
    key_id: U16<NativeEndian>,
    /// Must be zero.
    _reserved0: U16<NativeEndian>,
    /// Must be zero.
    _reserved1: U64<NativeEndian>,
    /// Must be zero.
    _reserved2: U64<NativeEndian>,
}

#[cfg(test)]
mod tests {
    use super::*;
}
