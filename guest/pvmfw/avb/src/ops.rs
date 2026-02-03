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
    SlotVerifyError, SlotVerifyFlags, SlotVerifyResult,
};
use core::ffi::CStr;

pub(crate) struct PvmfwAvbVerifier<'a, 'key> {
    kernel: &'a [u8],
    initrd: Option<&'a [u8]>,
    trusted_keys: &'a [&'key [u8]],
    validated_key: Option<&'key [u8]>,
}

impl<'a, 'key> PvmfwAvbVerifier<'a, 'key> {
    pub(crate) fn new(
        kernel: &'a [u8],
        initrd: Option<&'a [u8]>,
        trusted_keys: &'a [&'key [u8]],
    ) -> Self {
        Self { kernel, initrd, trusted_keys, validated_key: None }
    }

    fn get_partition(&self, partition_name: &CStr) -> IoResult<&'a [u8]> {
        match partition_name.try_into()? {
            PartitionName::Kernel => Ok(self.kernel),
            PartitionName::InitrdNormal | PartitionName::InitrdDebug => {
                self.initrd.ok_or(IoError::NoSuchPartition)
            }
        }
    }

    /// Returns the key used during the last verification, if any.
    pub(crate) fn get_validated_vbmeta_key(&self) -> Option<&'key [u8]> {
        self.validated_key
    }

    pub(crate) fn verify_partition(
        &mut self,
        partition_name: PartitionName,
    ) -> SlotVerifyResult<SlotVerifyData<'a>> {
        self.verify_partition_impl(partition_name, None)
    }

    pub(crate) fn verify_sized_partition(
        &mut self,
        partition_name: PartitionName,
        expected_length: usize,
    ) -> SlotVerifyResult<SlotVerifyData<'a>> {
        self.verify_partition_impl(partition_name, Some(expected_length))
    }

    pub(crate) fn verify_partition_impl(
        &mut self,
        partition_name: PartitionName,
        expected_length: Option<usize>,
    ) -> SlotVerifyResult<SlotVerifyData<'a>> {
        // Note that this call manages to verify the initrd images using hashes contained in the
        // (unique) VBMeta from the end of self.kernel because if
        //
        // - read_from_partition("vbmeta") returns AVB_IO_RESULT_ERROR_NO_SUCH_PARTITION and
        // - we do NOT pass AVB_SLOT_VERIFY_FLAGS_NO_VBMETA_PARTITION to slot_verify()
        //
        // then libavb (specifically, avb_slot_verify()) falls back to retrieving VBMeta from the
        // footer of the "boot" partition i.e. self.kernel (see PartitionName::Kernel).
        let result = slot_verify(
            self,
            &[partition_name.as_c_str()],
            None, // No partition slot suffix.
            SlotVerifyFlags::AVB_SLOT_VERIFY_FLAGS_NONE,
            HashtreeErrorMode::AVB_HASHTREE_ERROR_MODE_RESTART_AND_INVALIDATE,
        )?;

        // Paranoid sanity checks.
        assert_eq!(result.partition_data().len(), 1, "too many partitions for {partition_name:?}");
        let partition_data = result.partition_data().first().unwrap();
        assert_eq!(partition_data.partition_name(), partition_name.as_c_str());

        assert_eq!(result.vbmeta_data().len(), 1, "too many vbmetas for {partition_name:?}");
        let vbmeta_data = result.vbmeta_data().first().unwrap();
        assert_eq!(vbmeta_data.partition_name(), PartitionName::Kernel.as_c_str());

        if let Some(len) = expected_length {
            if partition_data.data().len() != len {
                return Err(SlotVerifyError::Verification(Some(result)));
            }
        }

        Ok(result)
    }
}

impl<'a, 'key> avb::Ops<'a> for PvmfwAvbVerifier<'a, 'key> {
    fn read_from_partition(
        &mut self,
        partition: &CStr,
        offset: i64,
        buffer: &mut [u8],
    ) -> IoResult<usize> {
        let partition = self.get_partition(partition)?;
        copy_data_to_dst(partition, offset, buffer)?;
        Ok(buffer.len())
    }

    fn get_preloaded_partition(
        &mut self,
        partition: &CStr,
        num_bytes: usize,
    ) -> IoResult<&'a [u8]> {
        self.get_partition(partition)?.get(..num_bytes).ok_or(IoError::RangeOutsidePartition)
    }

    fn validate_vbmeta_public_key(
        &mut self,
        public_key: &[u8],
        public_key_metadata: Option<&[u8]>,
    ) -> IoResult<bool> {
        // AVF payloads are signed without pubkey metadata so ignore the argument.
        let _ = public_key_metadata;
        self.validated_key = self.trusted_keys.iter().find(|&k| public_key == *k).map(|v| &**v);
        Ok(self.validated_key.is_some())
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
        let partition = self.get_partition(partition)?;
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
