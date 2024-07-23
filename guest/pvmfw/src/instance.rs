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

//! Support for reading and writing to the instance.img.

use crate::dice::PartialInputs;
use crate::gpt;
use crate::gpt::Partition;
use crate::gpt::Partitions;
use bssl_avf::{self, hkdf, Aead, AeadContext, Digester};
use core::fmt;
use core::mem::size_of;
use diced_open_dice::DiceMode;
use diced_open_dice::Hash;
use diced_open_dice::Hidden;
use log::trace;
use uuid::Uuid;
use virtio_drivers::transport::{pci::bus::PciRoot, DeviceType, Transport};
use vmbase::util::ceiling_div;
use vmbase::virtio::pci::{PciTransportIterator, VirtIOBlk};
use vmbase::virtio::HalImpl;
use zerocopy::AsBytes;
use zerocopy::FromBytes;
use zerocopy::FromZeroes;

pub enum Error {
    /// Unexpected I/O error while accessing the underlying disk.
    FailedIo(gpt::Error),
    /// Impossible to create a new instance.img entry.
    InstanceImageFull,
    /// Badly formatted instance.img header block.
    InvalidInstanceImageHeader,
    /// No instance.img ("vm-instance") partition found.
    MissingInstanceImage,
    /// The instance.img doesn't contain a header.
    MissingInstanceImageHeader,
    /// Authority hash found in the pvmfw instance.img entry doesn't match the trusted public key.
    RecordedAuthHashMismatch,
    /// Code hash found in the pvmfw instance.img entry doesn't match the inputs.
    RecordedCodeHashMismatch,
    /// DICE mode found in the pvmfw instance.img entry doesn't match the current one.
    RecordedDiceModeMismatch,
    /// Size of the instance.img entry being read or written is not supported.
    UnsupportedEntrySize(usize),
    /// Failed to create VirtIO Block device.
    VirtIOBlkCreationFailed(virtio_drivers::Error),
    /// An error happened during the interaction with BoringSSL.
    BoringSslFailed(bssl_avf::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::FailedIo(e) => write!(f, "Failed I/O to disk: {e}"),
            Self::InstanceImageFull => write!(f, "Failed to obtain a free instance.img partition"),
            Self::InvalidInstanceImageHeader => write!(f, "instance.img header is invalid"),
            Self::MissingInstanceImage => write!(f, "Failed to find the instance.img partition"),
            Self::MissingInstanceImageHeader => write!(f, "instance.img header is missing"),
            Self::RecordedAuthHashMismatch => write!(f, "Recorded authority hash doesn't match"),
            Self::RecordedCodeHashMismatch => write!(f, "Recorded code hash doesn't match"),
            Self::RecordedDiceModeMismatch => write!(f, "Recorded DICE mode doesn't match"),
            Self::UnsupportedEntrySize(sz) => write!(f, "Invalid entry size: {sz}"),
            Self::VirtIOBlkCreationFailed(e) => {
                write!(f, "Failed to create VirtIO Block device: {e}")
            }
            Self::BoringSslFailed(e) => {
                write!(f, "An error happened during the interaction with BoringSSL: {e}")
            }
        }
    }
}

impl From<bssl_avf::Error> for Error {
    fn from(e: bssl_avf::Error) -> Self {
        Self::BoringSslFailed(e)
    }
}

pub type Result<T> = core::result::Result<T, Error>;

fn aead_ctx_from_secret(secret: &[u8]) -> Result<AeadContext> {
    let key = hkdf::<32>(secret, /* salt= */ &[], b"vm-instance", Digester::sha512())?;
    Ok(AeadContext::new(Aead::aes_256_gcm_randnonce(), key.as_slice(), /* tag_len */ None)?)
}

/// Get the entry from instance.img. This method additionally returns Partition corresponding to
/// pvmfw in the instance.img as well as index corresponding to empty header which can be used to
/// record instance data with `record_instance_entry`.
pub(crate) fn get_recorded_entry(
    pci_root: &mut PciRoot,
    secret: &[u8],
) -> Result<(Option<EntryBody>, Partition, usize)> {
    let mut instance_img = find_instance_img(pci_root)?;

    let entry = locate_entry(&mut instance_img)?;
    trace!("Found pvmfw instance.img entry: {entry:?}");

    match entry {
        PvmfwEntry::Existing { header_index, payload_size } => {
            let aead_ctx = aead_ctx_from_secret(secret)?;
            let mut blk = [0; BLK_SIZE];
            if payload_size > blk.len() {
                // We currently only support single-blk entries.
                return Err(Error::UnsupportedEntrySize(payload_size));
            }
            let payload_index = header_index + 1;
            instance_img.read_block(payload_index, &mut blk).map_err(Error::FailedIo)?;

            let payload = &blk[..payload_size];
            let mut entry = [0; size_of::<EntryBody>()];
            // The nonce is generated internally for `aes_256_gcm_randnonce`, so no additional
            // nonce is required.
            let decrypted =
                aead_ctx.open(payload, /* nonce */ &[], /* ad */ &[], &mut entry)?;
            let body = EntryBody::read_from(decrypted).unwrap();
            Ok((Some(body), instance_img, header_index))
        }
        PvmfwEntry::New { header_index } => Ok((None, instance_img, header_index)),
    }
}

pub(crate) fn record_instance_entry(
    body: &EntryBody,
    secret: &[u8],
    instance_img: &mut Partition,
    header_index: usize,
) -> Result<()> {
    // We currently only support single-blk entries.
    let mut blk = [0; BLK_SIZE];
    let plaintext = body.as_bytes();
    let aead_ctx = aead_ctx_from_secret(secret)?;
    assert!(plaintext.len() + aead_ctx.aead().max_overhead() < blk.len());
    let encrypted = aead_ctx.seal(plaintext, /* nonce */ &[], /* ad */ &[], &mut blk)?;
    let payload_size = encrypted.len();
    let payload_index = header_index + 1;
    instance_img.write_block(payload_index, &blk).map_err(Error::FailedIo)?;

    let header = EntryHeader::new(PvmfwEntry::UUID, payload_size);
    header.write_to_prefix(blk.as_mut_slice()).unwrap();
    blk[header.as_bytes().len()..].fill(0);
    instance_img.write_block(header_index, &blk).map_err(Error::FailedIo)?;

    Ok(())
}

#[derive(FromZeroes, FromBytes)]
#[repr(C, packed)]
struct Header {
    magic: [u8; Header::MAGIC.len()],
    version: u16,
}

impl Header {
    const MAGIC: &'static [u8] = b"Android-VM-instance";
    const VERSION_1: u16 = 1;

    pub fn is_valid(&self) -> bool {
        self.magic == Self::MAGIC && self.version() == Self::VERSION_1
    }

    fn version(&self) -> u16 {
        u16::from_le(self.version)
    }
}

fn find_instance_img(pci_root: &mut PciRoot) -> Result<Partition> {
    for transport in PciTransportIterator::<HalImpl>::new(pci_root)
        .filter(|t| DeviceType::Block == t.device_type())
    {
        let device =
            VirtIOBlk::<HalImpl>::new(transport).map_err(Error::VirtIOBlkCreationFailed)?;
        match Partition::get_by_name(device, "vm-instance") {
            Ok(Some(p)) => return Ok(p),
            Ok(None) => {}
            Err(e) => log::warn!("error while reading from disk: {e}"),
        };
    }

    Err(Error::MissingInstanceImage)
}

#[derive(Debug)]
enum PvmfwEntry {
    Existing { header_index: usize, payload_size: usize },
    New { header_index: usize },
}

const BLK_SIZE: usize = Partitions::LBA_SIZE;

impl PvmfwEntry {
    const UUID: Uuid = Uuid::from_u128(0x90d2174a038a4bc6adf3824848fc5825);
}

fn locate_entry(partition: &mut Partition) -> Result<PvmfwEntry> {
    let mut blk = [0; BLK_SIZE];
    let mut indices = partition.indices();
    let header_index = indices.next().ok_or(Error::MissingInstanceImageHeader)?;
    partition.read_block(header_index, &mut blk).map_err(Error::FailedIo)?;
    // The instance.img header is only used for discovery/validation.
    let header = Header::read_from_prefix(blk.as_slice()).unwrap();
    if !header.is_valid() {
        return Err(Error::InvalidInstanceImageHeader);
    }

    while let Some(header_index) = indices.next() {
        partition.read_block(header_index, &mut blk).map_err(Error::FailedIo)?;

        let header = EntryHeader::read_from_prefix(blk.as_slice()).unwrap();
        match (header.uuid(), header.payload_size()) {
            (uuid, _) if uuid.is_nil() => return Ok(PvmfwEntry::New { header_index }),
            (PvmfwEntry::UUID, payload_size) => {
                return Ok(PvmfwEntry::Existing { header_index, payload_size })
            }
            (uuid, payload_size) => {
                trace!("Skipping instance.img entry {uuid}: {payload_size:?} bytes");
                let n = ceiling_div(payload_size, BLK_SIZE).unwrap();
                if n > 0 {
                    let _ = indices.nth(n - 1); // consume
                }
            }
        };
    }

    Err(Error::InstanceImageFull)
}

/// Marks the start of an instance.img entry.
///
/// Note: Virtualization/guest/microdroid_manager/src/instance.rs uses the name "partition".
#[derive(AsBytes, FromZeroes, FromBytes)]
#[repr(C, packed)]
struct EntryHeader {
    uuid: u128,
    payload_size: u64,
}

impl EntryHeader {
    fn new(uuid: Uuid, payload_size: usize) -> Self {
        Self { uuid: uuid.to_u128_le(), payload_size: u64::try_from(payload_size).unwrap().to_le() }
    }

    fn uuid(&self) -> Uuid {
        Uuid::from_u128_le(self.uuid)
    }

    fn payload_size(&self) -> usize {
        usize::try_from(u64::from_le(self.payload_size)).unwrap()
    }
}

#[derive(AsBytes, FromZeroes, FromBytes)]
#[repr(C)]
pub(crate) struct EntryBody {
    pub code_hash: Hash,
    pub auth_hash: Hash,
    pub salt: Hidden,
    mode: u8,
}

impl EntryBody {
    pub(crate) fn new(dice_inputs: &PartialInputs, salt: &Hidden) -> Self {
        let mode = match dice_inputs.mode {
            DiceMode::kDiceModeNotInitialized => 0,
            DiceMode::kDiceModeNormal => 1,
            DiceMode::kDiceModeDebug => 2,
            DiceMode::kDiceModeMaintenance => 3,
        };

        Self {
            code_hash: dice_inputs.code_hash,
            auth_hash: dice_inputs.auth_hash,
            salt: *salt,
            mode,
        }
    }

    pub(crate) fn mode(&self) -> DiceMode {
        match self.mode {
            1 => DiceMode::kDiceModeNormal,
            2 => DiceMode::kDiceModeDebug,
            3 => DiceMode::kDiceModeMaintenance,
            _ => DiceMode::kDiceModeNotInitialized,
        }
    }
}
