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

//! Structs and functions relating to AVB callback operations.

use crate::partition::PartitionName;
use avb::{
    slot_verify, HashtreeErrorMode, IoError, IoResult, PublicKeyForPartitionInfo, SlotVerifyData,
    SlotVerifyFlags, SlotVerifyResult,
};
use core::ffi::CStr;

pub(crate) struct Payload<'a> {
    kernel: &'a [u8],
    initrd: Option<&'a [u8]>,
    trusted_public_key: &'a [u8],
}

impl<'a> Payload<'a> {
    pub(crate) fn new(
        kernel: &'a [u8],
        initrd: Option<&'a [u8]>,
        trusted_public_key: &'a [u8],
    ) -> Self {
        Self { kernel, initrd, trusted_public_key }
    }

    fn get_partition(&self, partition_name: &CStr) -> IoResult<&[u8]> {
        match partition_name.try_into()? {
            PartitionName::Kernel => Ok(self.kernel),
            PartitionName::InitrdNormal | PartitionName::InitrdDebug => {
                self.initrd.ok_or(IoError::NoSuchPartition)
            }
        }
    }
}

/// Pvmfw customized operations used in the verification.
pub(crate) struct Ops<'a> {
    payload: &'a Payload<'a>,
}

impl<'a> Ops<'a> {
    pub(crate) fn new(payload: &'a Payload<'a>) -> Self {
        Self { payload }
    }

    pub(crate) fn verify_partition(
        &mut self,
        partition_name: &CStr,
    ) -> SlotVerifyResult<SlotVerifyData<'a>> {
        slot_verify(
            self,
            &[partition_name],
            None, // No partition slot suffix.
            SlotVerifyFlags::AVB_SLOT_VERIFY_FLAGS_NONE,
            HashtreeErrorMode::AVB_HASHTREE_ERROR_MODE_RESTART_AND_INVALIDATE,
        )
    }
}

impl<'a> avb::Ops<'a> for Ops<'a> {
    fn read_from_partition(
        &mut self,
        partition: &CStr,
        offset: i64,
        buffer: &mut [u8],
    ) -> IoResult<usize> {
        let partition = self.payload.get_partition(partition)?;
        copy_data_to_dst(partition, offset, buffer)?;
        Ok(buffer.len())
    }

    fn get_preloaded_partition(&mut self, partition: &CStr) -> IoResult<&'a [u8]> {
        self.payload.get_partition(partition)
    }

    fn validate_vbmeta_public_key(
        &mut self,
        public_key: &[u8],
        _public_key_metadata: Option<&[u8]>,
    ) -> IoResult<bool> {
        // The public key metadata is not used when we build the VBMeta.
        Ok(self.payload.trusted_public_key == public_key)
    }

    fn read_rollback_index(&mut self, _rollback_index_location: usize) -> IoResult<u64> {
        // TODO(291213394) : Refine this comment once capability for rollback protection is defined.
        // pvmfw does not compare stored_rollback_index with rollback_index for Antirollback
        // protection. Hence, we set `out_rollback_index` to 0 to ensure that the rollback_index
        // (including default: 0) is never smaller than it, thus the rollback index check will pass.
        Ok(0)
    }

    fn write_rollback_index(
        &mut self,
        _rollback_index_location: usize,
        _index: u64,
    ) -> IoResult<()> {
        Err(IoError::NotImplemented)
    }

    fn read_is_device_unlocked(&mut self) -> IoResult<bool> {
        Ok(false)
    }

    fn get_size_of_partition(&mut self, partition: &CStr) -> IoResult<u64> {
        let partition = self.payload.get_partition(partition)?;
        u64::try_from(partition.len()).map_err(|_| IoError::InvalidValueSize)
    }

    fn read_persistent_value(&mut self, _name: &CStr, _value: &mut [u8]) -> IoResult<usize> {
        Err(IoError::NotImplemented)
    }

    fn write_persistent_value(&mut self, _name: &CStr, _value: &[u8]) -> IoResult<()> {
        Err(IoError::NotImplemented)
    }

    fn erase_persistent_value(&mut self, _name: &CStr) -> IoResult<()> {
        Err(IoError::NotImplemented)
    }

    fn validate_public_key_for_partition(
        &mut self,
        _partition: &CStr,
        _public_key: &[u8],
        _public_key_metadata: Option<&[u8]>,
    ) -> IoResult<PublicKeyForPartitionInfo> {
        Err(IoError::NotImplemented)
    }
}

fn copy_data_to_dst(src: &[u8], offset: i64, dst: &mut [u8]) -> IoResult<()> {
    let start = to_copy_start(offset, src.len()).ok_or(IoError::InvalidValueSize)?;
    let end = start.checked_add(dst.len()).ok_or(IoError::InvalidValueSize)?;
    dst.copy_from_slice(src.get(start..end).ok_or(IoError::RangeOutsidePartition)?);
    Ok(())
}

fn to_copy_start(offset: i64, len: usize) -> Option<usize> {
    usize::try_from(offset)
        .ok()
        .or_else(|| isize::try_from(offset).ok().and_then(|v| len.checked_add_signed(v)))
}
