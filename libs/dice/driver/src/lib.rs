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

//! Logic for handling the DICE values and boot operations.

use anyhow::{anyhow, bail, Context, Error, Result};
use byteorder::{NativeEndian, ReadBytesExt};
use diced_open_dice::{
    bcc_handover_parse, retry_bcc_main_flow, BccHandover, Config, DiceArtifacts, DiceMode, Hash,
    Hidden, InputValues, OwnedDiceArtifacts,
};
use keystore2_crypto::ZVec;
use libc::{c_void, mmap, munmap, MAP_FAILED, MAP_PRIVATE, PROT_READ};
use openssl::hkdf::hkdf;
use openssl::md::Md;
use std::fs;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::slice;

/// Artifacts that are mapped into the process address space from the driver.
pub enum DiceDriver<'a> {
    /// Implementation that reads bcc handover from the dice driver.
    Real {
        /// Path to the driver character device (e.g. /dev/open-dice0).
        driver_path: PathBuf,
        /// Address of the memory to mmap driver to.
        mmap_addr: *mut c_void,
        /// Size of the mmap.
        mmap_size: usize,
        /// BCC handover.
        bcc_handover: BccHandover<'a>,
    },
    /// Fake implementation used in tests and non-protected VMs.
    Fake(OwnedDiceArtifacts),
    /// Implementation that reads bcc handover from the file.
    FromFile {
        /// Path to the file to read dice chain from,
        file_path: PathBuf,
        /// Dice artifacts read from file_path,
        dice_artifacts: OwnedDiceArtifacts,
    },
}

impl DiceDriver<'_> {
    fn dice_artifacts(&self) -> &dyn DiceArtifacts {
        match self {
            Self::Real { bcc_handover, .. } => bcc_handover,
            Self::Fake(owned_dice_artifacts) => owned_dice_artifacts,
            Self::FromFile { dice_artifacts, .. } => dice_artifacts,
        }
    }

    /// Creates a new dice driver from the given driver_path.
    pub fn new(driver_path: &Path, is_strict_boot: bool) -> Result<Self> {
        log::info!("Creating DiceDriver backed by {driver_path:?} driver");
        if driver_path.exists() {
            log::info!("Using DICE values from driver");
        } else if is_strict_boot {
            bail!("Strict boot requires DICE value from driver but none were found");
        } else {
            log::warn!("Using sample DICE values");
            let dice_artifacts = diced_sample_inputs::make_sample_bcc_and_cdis()
                .expect("Failed to create sample dice artifacts.");
            return Ok(Self::Fake(dice_artifacts));
        };

        let mut file = fs::File::open(driver_path)
            .map_err(|error| Error::new(error).context("Opening driver"))?;
        let mmap_size =
            file.read_u64::<NativeEndian>()
                .map_err(|error| Error::new(error).context("Reading driver"))? as usize;
        // SAFETY: It's safe to map the driver as the service will only create a single
        // mapping per process.
        let mmap_addr = unsafe {
            let fd = file.as_raw_fd();
            mmap(null_mut(), mmap_size, PROT_READ, MAP_PRIVATE, fd, 0)
        };
        if mmap_addr == MAP_FAILED {
            bail!("Failed to mmap {:?}", driver_path);
        }
        let mmap_buf =
        // SAFETY: The slice is created for the region of memory that was just
        // successfully mapped into the process address space so it will be
        // accessible and not referenced from anywhere else.
            unsafe { slice::from_raw_parts((mmap_addr as *const u8).as_ref().unwrap(), mmap_size) };
        let bcc_handover =
            bcc_handover_parse(mmap_buf).map_err(|_| anyhow!("Failed to parse Bcc Handover"))?;
        Ok(Self::Real {
            driver_path: driver_path.to_path_buf(),
            mmap_addr,
            mmap_size,
            bcc_handover,
        })
    }

    /// Create a new dice driver that reads dice_artifacts from the given file.
    pub fn from_file(file_path: &Path) -> Result<Self> {
        log::info!("Creating DiceDriver backed by {file_path:?} file");
        let file =
            fs::File::open(file_path).map_err(|error| Error::new(error).context("open file"))?;
        let dice_artifacts = serde_cbor::from_reader(file)
            .map_err(|error| Error::new(error).context("read file"))?;
        Ok(Self::FromFile { file_path: file_path.to_path_buf(), dice_artifacts })
    }

    /// Derives a sealing key of `key_length` bytes from the DICE sealing CDI.
    pub fn get_sealing_key(&self, identifier: &[u8], key_length: usize) -> Result<ZVec> {
        // Deterministically derive a key to use for sealing data, rather than using the CDI
        // directly, so we have the chance to rotate the key if needed. A salt isn't needed as the
        // input key material is already cryptographically strong.
        let mut key = ZVec::new(key_length)?;
        let salt = &[];
        hkdf(&mut key, Md::sha256(), self.dice_artifacts().cdi_seal(), salt, identifier)?;
        Ok(key)
    }

    /// Derives a new dice chain.
    pub fn derive(
        self,
        code_hash: Hash,
        config_desc: &[u8],
        authority_hash: Hash,
        debug: bool,
        hidden: Hidden,
    ) -> Result<OwnedDiceArtifacts> {
        let input_values = InputValues::new(
            code_hash,
            Config::Descriptor(config_desc),
            authority_hash,
            if debug { DiceMode::kDiceModeDebug } else { DiceMode::kDiceModeNormal },
            hidden,
        );
        let current_dice_artifacts = self.dice_artifacts();
        let next_dice_artifacts = retry_bcc_main_flow(
            current_dice_artifacts.cdi_attest(),
            current_dice_artifacts.cdi_seal(),
            current_dice_artifacts.bcc().ok_or_else(|| anyhow!("bcc is none"))?,
            &input_values,
        )
        .context("DICE derive from driver")?;
        match &self {
            Self::Real { driver_path, .. } => {
                // Writing to the device wipes the artifacts. The string is ignored by the driver
                // but included for documentation.
                fs::write(driver_path, "wipe")
                    .map_err(|err| Error::new(err).context("Wiping driver"))?;
            }
            Self::FromFile { file_path, .. } => {
                fs::remove_file(file_path)
                    .map_err(|err| Error::new(err).context("Deleting file"))?;
            }
            Self::Fake { .. } => (),
        }
        Ok(next_dice_artifacts)
    }
}

impl Drop for DiceDriver<'_> {
    fn drop(&mut self) {
        if let &mut Self::Real { mmap_addr, mmap_size, .. } = self {
            // SAFETY: All references to the mapped region have the same lifetime as self. Since
            // self is being dropped, so are all the references to the mapped region meaning it's
            // safe to unmap.
            let ret = unsafe { munmap(mmap_addr, mmap_size) };
            if ret != 0 {
                log::warn!("Failed to munmap ({})", ret);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::CStr;
    use diced_open_dice::{
        hash, retry_bcc_format_config_descriptor, DiceConfigValues, HIDDEN_SIZE,
    };
    use std::fs::File;

    fn assert_eq_bytes(expected: &[u8], actual: &[u8]) {
        assert_eq!(
            expected,
            actual,
            "Expected {}, got {}",
            hex::encode(expected),
            hex::encode(actual)
        )
    }

    #[test]
    fn test_write_bcc_to_file_read_from_file() -> Result<()> {
        let dice_artifacts = diced_sample_inputs::make_sample_bcc_and_cdis()?;

        let test_file = tempfile::NamedTempFile::new()?;
        serde_cbor::to_writer(test_file.as_file(), &dice_artifacts)?;
        test_file.as_file().sync_all()?;

        let dice = DiceDriver::from_file(test_file.as_ref())?;

        let dice_artifacts2 = dice.dice_artifacts();
        assert_eq_bytes(dice_artifacts.cdi_attest(), dice_artifacts2.cdi_attest());
        assert_eq_bytes(dice_artifacts.cdi_seal(), dice_artifacts2.cdi_seal());
        assert_eq_bytes(dice_artifacts.bcc().expect("bcc"), dice_artifacts2.bcc().expect("bcc"));

        Ok(())
    }

    #[test]
    fn test_dice_driver_from_file_deletes_file_after_derive() -> Result<()> {
        let tmp_dir = tempfile::tempdir()?;

        let file_path = tmp_dir.path().join("test-dice-chain.raw");

        {
            let dice_artifacts = diced_sample_inputs::make_sample_bcc_and_cdis()?;
            let file = File::create(&file_path)?;
            serde_cbor::to_writer(file, &dice_artifacts)?;
        }

        let dice = DiceDriver::from_file(&file_path)?;

        let values = DiceConfigValues {
            component_name: Some(CStr::from_bytes_with_nul(b"test\0")?),
            ..Default::default()
        };
        let desc = retry_bcc_format_config_descriptor(&values)?;
        let code_hash = hash(&String::from("test code hash").into_bytes())?;
        let authority_hash = hash(&String::from("test authority hash").into_bytes())?;
        let hidden = [0; HIDDEN_SIZE];

        let _ = dice.derive(code_hash, &desc, authority_hash, false, hidden)?;

        assert!(!file_path.exists());

        Ok(())
    }
}
