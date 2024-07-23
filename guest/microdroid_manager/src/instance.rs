// Copyright 2021, The Android Open Source Project
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

//! Provides routines to read/write on the instance disk.
//!
//! Instance disk is a disk where the identity of a VM instance is recorded. The identity usually
//! includes certificates of the VM payload that is trusted, but not limited to it. Instance disk
//! is empty when a VM is first booted. The identity data is filled in during the first boot, and
//! then encrypted and signed. Subsequent boots decrypts and authenticates the data and uses the
//! identity data to further verify the payload (e.g. against the certificate).
//!
//! Instance disk consists of a disk header and one or more partitions each of which consists of a
//! header and payload. Each header (both the disk header and a partition header) is 512 bytes
//! long. Payload is just next to the header and its size can be arbitrary. Headers are located at
//! 512 bytes boundaries. So, when the size of a payload is not multiple of 512, there exists a gap
//! between the end of the payload and the start of the next partition (if there is any).
//!
//! Each partition is identified by a UUID. A partition is created for a program loader that
//! participates in the boot chain of the VM. Each program loader is expected to locate the
//! partition that corresponds to the loader using the UUID that is assigned to the loader.
//!
//! The payload of a partition is encrypted/signed by a key that is unique to the loader and to the
//! VM as well. Failing to decrypt/authenticate a partition by a loader stops the boot process.

use crate::ioutil;

use anyhow::{anyhow, bail, Context, Result};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use dice_driver::DiceDriver;
use openssl::symm::{decrypt_aead, encrypt_aead, Cipher};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use uuid::Uuid;

/// Path to the instance disk inside the VM
const INSTANCE_IMAGE_PATH: &str = "/dev/block/by-name/vm-instance";

/// Identifier for the key used to seal the instance data.
const INSTANCE_KEY_IDENTIFIER: &[u8] = b"microdroid_manager_key";

/// Magic string in the instance disk header
const DISK_HEADER_MAGIC: &str = "Android-VM-instance";

/// Version of the instance disk format
const DISK_HEADER_VERSION: u16 = 1;

/// Size of the headers in the instance disk
const DISK_HEADER_SIZE: u64 = 512;
const PARTITION_HEADER_SIZE: u64 = 512;

/// UUID of the partition that microdroid manager uses
const MICRODROID_PARTITION_UUID: &str = "cf9afe9a-0662-11ec-a329-c32663a09d75";

/// Size of the AES256-GCM tag
const AES_256_GCM_TAG_LENGTH: usize = 16;

/// Size of the AES256-GCM nonce
const AES_256_GCM_NONCE_LENGTH: usize = 12;

/// Handle to the instance disk
pub struct InstanceDisk {
    file: File,
}

/// Information from a partition header
struct PartitionHeader {
    uuid: Uuid,
    payload_size: u64, // in bytes
}

/// Offset of a partition in the instance disk
type PartitionOffset = u64;

impl InstanceDisk {
    /// Creates handle to instance disk
    pub fn new() -> Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(INSTANCE_IMAGE_PATH)
            .with_context(|| format!("Failed to open {}", INSTANCE_IMAGE_PATH))?;

        // Check if this file is a valid instance disk by examining the header (the first block)
        let mut magic = [0; DISK_HEADER_MAGIC.len()];
        file.read_exact(&mut magic)?;
        if magic != DISK_HEADER_MAGIC.as_bytes() {
            bail!("invalid magic: {:?}", magic);
        }

        let version = file.read_u16::<LittleEndian>()?;
        if version == 0 {
            bail!("invalid version: {}", version);
        }
        if version > DISK_HEADER_VERSION {
            bail!("unsupported version: {}", version);
        }

        Ok(Self { file })
    }

    /// Reads the identity data that was written by microdroid manager. The returned data is
    /// plaintext, although it is stored encrypted. In case when the partition for microdroid
    /// manager doesn't exist, which can happen if it's the first boot, `Ok(None)` is returned.
    pub fn read_microdroid_data(&mut self, dice: &DiceDriver) -> Result<Option<MicrodroidData>> {
        let (header, offset) = self.locate_microdroid_header()?;
        if header.is_none() {
            return Ok(None);
        }
        let header = header.unwrap();
        let payload_offset = offset + PARTITION_HEADER_SIZE;
        self.file.seek(SeekFrom::Start(payload_offset))?;

        // Read the nonce (unencrypted)
        let mut nonce = [0; AES_256_GCM_NONCE_LENGTH];
        self.file.read_exact(&mut nonce)?;

        // Read the encrypted payload
        let payload_size =
            header.payload_size as usize - AES_256_GCM_NONCE_LENGTH - AES_256_GCM_TAG_LENGTH;
        let mut data = vec![0; payload_size];
        self.file.read_exact(&mut data)?;

        // Read the tag
        let mut tag = [0; AES_256_GCM_TAG_LENGTH];
        self.file.read_exact(&mut tag)?;

        // Read the header as well because it's part of the signed data (though not encrypted).
        let mut header = [0; PARTITION_HEADER_SIZE as usize];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut header)?;

        // Decrypt and authenticate the data (along with the header).
        let cipher = Cipher::aes_256_gcm();
        let key = dice.get_sealing_key(INSTANCE_KEY_IDENTIFIER, cipher.key_len())?;
        let plaintext = decrypt_aead(cipher, &key, Some(&nonce), &header, &data, &tag)?;

        let microdroid_data = serde_cbor::from_slice(plaintext.as_slice())?;
        Ok(Some(microdroid_data))
    }

    /// Writes identity data to the partition for microdroid manager. The partition is appended
    /// if it doesn't exist. The data is stored encrypted.
    pub fn write_microdroid_data(
        &mut self,
        microdroid_data: &MicrodroidData,
        dice: &DiceDriver,
    ) -> Result<()> {
        let (header, offset) = self.locate_microdroid_header()?;

        let data = serde_cbor::to_vec(microdroid_data)?;

        // By encrypting and signing the data, tag will be appended. The tag also becomes part of
        // the encrypted payload which will be written. In addition, a nonce will be prepended
        // (non-encrypted).
        let payload_size = (AES_256_GCM_NONCE_LENGTH + data.len() + AES_256_GCM_TAG_LENGTH) as u64;

        // If the partition exists, make sure we don't change the partition size. If not (i.e.
        // partition is not found), write the header at the empty place.
        if let Some(header) = header {
            if header.payload_size != payload_size {
                bail!("Can't change payload size from {} to {}", header.payload_size, payload_size);
            }
        } else {
            let uuid = Uuid::parse_str(MICRODROID_PARTITION_UUID)?;
            self.write_header_at(offset, &uuid, payload_size)?;
        }

        // Read the header as it is used as additionally authenticated data (AAD).
        let mut header = [0; PARTITION_HEADER_SIZE as usize];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut header)?;

        // Generate a nonce randomly and recorde it on the disk first.
        let nonce = rand::random::<[u8; AES_256_GCM_NONCE_LENGTH]>();
        self.file.seek(SeekFrom::Start(offset + PARTITION_HEADER_SIZE))?;
        self.file.write_all(nonce.as_ref())?;

        // Then encrypt and sign the data.
        let cipher = Cipher::aes_256_gcm();
        let key = dice.get_sealing_key(INSTANCE_KEY_IDENTIFIER, cipher.key_len())?;
        let mut tag = [0; AES_256_GCM_TAG_LENGTH];
        let ciphertext = encrypt_aead(cipher, &key, Some(&nonce), &header, &data, &mut tag)?;

        // Persist the encrypted payload data and the tag.
        self.file.write_all(&ciphertext)?;
        self.file.write_all(&tag)?;
        ioutil::blkflsbuf(&mut self.file)?;

        Ok(())
    }

    /// Read header at `header_offset` and parse it into a `PartitionHeader`.
    fn read_header_at(&mut self, header_offset: u64) -> Result<PartitionHeader> {
        assert!(
            header_offset % PARTITION_HEADER_SIZE == 0,
            "header offset {} is not aligned to 512 bytes",
            header_offset
        );

        let mut uuid = [0; 16];
        self.file.seek(SeekFrom::Start(header_offset))?;
        self.file.read_exact(&mut uuid)?;
        let uuid = Uuid::from_bytes(uuid);
        let payload_size = self.file.read_u64::<LittleEndian>()?;

        Ok(PartitionHeader { uuid, payload_size })
    }

    /// Write header at `header_offset`
    fn write_header_at(
        &mut self,
        header_offset: u64,
        uuid: &Uuid,
        payload_size: u64,
    ) -> Result<()> {
        self.file.seek(SeekFrom::Start(header_offset))?;
        self.file.write_all(uuid.as_bytes())?;
        self.file.write_u64::<LittleEndian>(payload_size)?;
        Ok(())
    }

    /// Locate the header of the partition for microdroid manager. A pair of `PartitionHeader` and
    /// the offset of the partition in the disk is returned. If the partition is not found,
    /// `PartitionHeader` is `None` and the offset points to the empty partition that can be used
    /// for the partition.
    fn locate_microdroid_header(&mut self) -> Result<(Option<PartitionHeader>, PartitionOffset)> {
        let microdroid_uuid = Uuid::parse_str(MICRODROID_PARTITION_UUID)?;

        // the first partition header is located just after the disk header
        let mut header_offset = DISK_HEADER_SIZE;
        loop {
            let header = self.read_header_at(header_offset)?;
            if header.uuid == microdroid_uuid {
                // found a matching header
                return Ok((Some(header), header_offset));
            } else if header.uuid == Uuid::nil() {
                // found an empty space
                return Ok((None, header_offset));
            }
            // Move to the next partition. Be careful about overflow.
            let payload_size = round_to_multiple(header.payload_size, PARTITION_HEADER_SIZE)?;
            let part_size = payload_size
                .checked_add(PARTITION_HEADER_SIZE)
                .ok_or_else(|| anyhow!("partition too large"))?;
            header_offset = header_offset
                .checked_add(part_size)
                .ok_or_else(|| anyhow!("next partition at invalid offset"))?;
        }
    }
}

/// Round `n` up to the nearest multiple of `unit`
fn round_to_multiple(n: u64, unit: u64) -> Result<u64> {
    assert!((unit & (unit - 1)) == 0, "{} is not power of two", unit);
    let ret = (n + unit - 1) & !(unit - 1);
    if ret < n {
        bail!("overflow")
    }
    Ok(ret)
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MicrodroidData {
    // `salt` is obsolete, it was used as a differentiator for non-protected VM instances running
    // same payload. Instance-id (present in DT) is used for that now.
    pub salt: Vec<u8>, // Should be [u8; 64] but that isn't serializable.
    pub apk_data: ApkData,
    pub extra_apks_data: Vec<ApkData>,
    pub apex_data: Vec<ApexData>,
}

impl MicrodroidData {
    pub fn extra_apk_root_hash_eq(&self, i: usize, root_hash: &[u8]) -> bool {
        self.extra_apks_data.get(i).map_or(false, |apk| apk.root_hash_eq(root_hash))
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApkData {
    pub root_hash: Vec<u8>,
    pub cert_hash: Vec<u8>,
    pub package_name: String,
    pub version_code: u64,
}

impl ApkData {
    pub fn root_hash_eq(&self, root_hash: &[u8]) -> bool {
        self.root_hash == root_hash
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApexData {
    pub name: String,
    pub manifest_name: Option<String>,
    pub manifest_version: Option<i64>,
    pub public_key: Vec<u8>,
    pub root_digest: Vec<u8>,
    pub last_update_seconds: u64,
    pub is_factory: bool,
}
