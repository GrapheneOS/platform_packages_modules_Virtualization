// Copyright 2024, The Android Open Source Project
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

//! Support for guest-specific rollback protection (RBP).

use crate::config::RollbackConfig;
use crate::config::RollbackConfigEntry;
use crate::config::RollbackConfigKeyId;
use crate::config::RollbackConfigPolicy;
use crate::dice::PartialInputs;
use crate::entry::RebootReason;
use crate::fdt::read_defer_rollback_protection;
use crate::instance::EntryBody;
use crate::instance::Error as InstanceError;
use crate::instance::{get_recorded_entry, record_instance_entry};
use crate::PUBLIC_KEY;
use bssl_crypto::digest::Sha512;
use diced_open_dice::Hidden;
use libfdt::Fdt;
use log::{error, info, warn};
use pvmfw_avb::Capability;
use pvmfw_avb::Digest;
use pvmfw_avb::VerifiedBootData;
use virtio_drivers::transport::pci::bus::{ConfigurationAccess, PciRoot};
#[cfg(target_arch = "x86_64")]
use vmbase::acpi::pci::initialize_from_acpi;
#[cfg(target_arch = "aarch64")]
use vmbase::fdt::pci::initialize_from_fdt;
use vmbase::rand;

/// Criteria hard-coded into pvmfw, to perform fixed image verification.
pub(crate) enum FixedRollbackCriterion<'a> {
    #[cfg_attr(not(feature = "platform_has_desktop_trusty"), allow(dead_code))]
    /// Image must match the exact kernel hash.
    KernelHash { digests: &'a [Digest] },
    /// Image must match the exact rollback index and have been signed with the given public key.
    RollbackIndexPublicKey { index: u64, public_key: &'a [u8] },
    #[cfg_attr(feature = "platform_has_desktop_trusty", allow(dead_code))]
    /// Image identifier is reserved but not supported on this platform so must be rejected.
    Reserved { name: &'a str },
}

/// Performs RBP based on the input payload, current DICE chain, and host-controlled platform.
///
/// On success, returns a tuple containing:
/// - `new_instance`: true if the legacy instance.img solution was used and a new entry created;
/// - `salt`: the salt representing the instance, to be used during DICE derivation;
/// - `defer_rollback_protection`: if RBP is being deferred.
pub fn perform_rollback_protection<'a>(
    fdt: &Fdt,
    verified_boot_data: &VerifiedBootData,
    dice_inputs: &PartialInputs,
    cdi_seal: &[u8],
    rollback_config: Option<&RollbackConfig<'a>>,
    extra_keys: &[&'a [u8]],
) -> Result<(bool, Hidden, bool), RebootReason> {
    let instance_hash = dice_inputs.instance_hash;
    if let Some(fixed) =
        get_fixed_rollback_protection(verified_boot_data, rollback_config, extra_keys)
    {
        // Prevent attackers from impersonating well-known images.
        perform_fixed_rollback_protection(verified_boot_data, fixed)?;
        Ok((false, instance_hash.unwrap(), false))
    } else if (should_defer_rollback_protection(fdt)?
        && verified_boot_data.has_capability(Capability::SecretkeeperProtection))
        || verified_boot_data.has_capability(Capability::TrustySecurityVm)
    {
        perform_deferred_rollback_protection(verified_boot_data)?;
        Ok((false, instance_hash.unwrap(), true))
    } else if cfg!(feature = "instance-img") {
        #[cfg(target_arch = "aarch64")]
        let pci_root = initialize_from_fdt(fdt);
        #[cfg(target_arch = "x86_64")]
        let pci_root = initialize_from_acpi();

        let pci_root = pci_root.map_err(|e| {
            error!("Failed to initialize PCI: {:?}", e);
            RebootReason::InternalError
        })?;
        perform_legacy_rollback_protection(pci_root, dice_inputs, cdi_seal)
    } else {
        force_new_instance()
    }
}

fn perform_deferred_rollback_protection(
    verified_boot_data: &VerifiedBootData,
) -> Result<(), RebootReason> {
    info!("Deferring rollback protection");
    // rollback_index of the image is used as security_version and is expected to be > 0 to
    // discourage implicit allocation.
    if verified_boot_data.rollback_index == 0 {
        error!("Expected positive rollback_index, found 0");
        Err(RebootReason::InvalidPayload)
    } else {
        Ok(())
    }
}

fn get_fixed_rollback_protection<'a>(
    verified_boot_data: &VerifiedBootData,
    rollback_config: Option<&'a RollbackConfig<'a>>,
    extra_keys: &[&'a [u8]],
) -> Option<FixedRollbackCriterion<'a>> {
    match verified_boot_data.name.as_deref()? {
        VerifiedBootData::RKP_VM_NAME => Some(FixedRollbackCriterion::RollbackIndexPublicKey {
            index: platform_security_patch_timestamp::TIMESTAMP,
            public_key: PUBLIC_KEY,
        }),
        VerifiedBootData::DESKTOP_TRUSTY_VM_NAME => {
            cfg_if::cfg_if! {
                if #[cfg(feature = "platform_has_desktop_trusty")] {
                    const MAIN_DIGEST: &Digest = include_bytes!(
                        concat!(env!("OUT_DIR"), "/desktop_trusty.kernelhash")
                    );
                    const ALT_DIGEST: &Digest = include_bytes!(
                        concat!(env!("OUT_DIR"), "/desktop_trusty_ext_boot.kernelhash")
                    );
                    static ALLOWED_DIGESTS: &[Digest] = &[*MAIN_DIGEST, *ALT_DIGEST];

                    static_assertions::const_assert!(MAIN_DIGEST.len() == pvmfw_avb::DIGEST_LEN);
                    static_assertions::const_assert!(ALT_DIGEST.len() == pvmfw_avb::DIGEST_LEN);

                    Some(FixedRollbackCriterion::KernelHash { digests: ALLOWED_DIGESTS })
                } else {
                    let name = VerifiedBootData::DESKTOP_TRUSTY_VM_NAME;
                    Some(FixedRollbackCriterion::Reserved { name })
                }
            }
        }
        name => {
            if let Some(config) = rollback_config {
                for entry in config.entries() {
                    let criterion = criterion_for_vm_from_config_entry(name, entry, extra_keys);
                    if criterion.is_some() {
                        return criterion;
                    }
                }
            }
            None
        }
    }
}

fn criterion_for_vm_from_config_entry<'a>(
    vm_name: &str,
    entry: &RollbackConfigEntry<'a>,
    extra_keys: &[&'a [u8]],
) -> Option<FixedRollbackCriterion<'a>> {
    if entry.vm_name != vm_name {
        return None;
    }
    let criterion = match entry.rollback_policy {
        RollbackConfigPolicy::Reserved => FixedRollbackCriterion::Reserved { name: entry.vm_name },
        RollbackConfigPolicy::MinimumRollbackIndex(
            index,
            RollbackConfigKeyId::EmbeddedPublicKey,
        ) => FixedRollbackCriterion::RollbackIndexPublicKey { index, public_key: PUBLIC_KEY },
        RollbackConfigPolicy::MinimumRollbackIndex(
            index,
            RollbackConfigKeyId::ExtraTrustedKey { n },
        ) if extra_keys.get(n).is_none() => {
            error!("Invalid key ID {n}: rejecting payload");
            FixedRollbackCriterion::Reserved { name: entry.vm_name }
        }
        RollbackConfigPolicy::MinimumRollbackIndex(
            index,
            RollbackConfigKeyId::ExtraTrustedKey { n },
        ) => FixedRollbackCriterion::RollbackIndexPublicKey { index, public_key: extra_keys[n] },
    };
    Some(criterion)
}

fn perform_fixed_rollback_protection(
    verified_boot_data: &VerifiedBootData,
    criterion: FixedRollbackCriterion,
) -> Result<(), RebootReason> {
    info!("Performing fixed rollback protection");
    match criterion {
        FixedRollbackCriterion::RollbackIndexPublicKey {
            index: fixed_index,
            public_key: expected_key,
        } => {
            let index = verified_boot_data.rollback_index;
            let public_key = verified_boot_data.public_key;
            if index != fixed_index {
                error!("Rollback index mismatch: expected {fixed_index}, found {index}");
                Err(RebootReason::InvalidPayload)
            } else if public_key != expected_key {
                error!("Public key mismatch: expected {expected_key:x?}, found {public_key:x?}");
                Err(RebootReason::InvalidPayload)
            } else {
                Ok(())
            }
        }
        FixedRollbackCriterion::KernelHash { digests } => {
            let digest = verified_boot_data.kernel_digest;
            if !digests.contains(&digest) {
                error!("Kernel hash mismatch: expected one of {digests:x?}, found {digest:x?}");
                Err(RebootReason::InvalidPayload)
            } else {
                Ok(())
            }
        }
        FixedRollbackCriterion::Reserved { name } => {
            error!("Reserved payload name \"{name}\" not supported.");
            Err(RebootReason::InvalidPayload)
        }
    }
}

/// Performs RBP using instance.img where updates require clearing old entries, causing new CDIs.
fn perform_legacy_rollback_protection(
    mut pci_root: PciRoot<impl ConfigurationAccess>,
    dice_inputs: &PartialInputs,
    cdi_seal: &[u8],
) -> Result<(bool, Hidden, bool), RebootReason> {
    info!("Fallback to instance.img based rollback checks");
    let result = get_recorded_entry(&mut pci_root, cdi_seal);
    if matches!(result, Err(InstanceError::MissingInstanceImage)) {
        warn!("instance.img is missing. Falling back to force_new_instance");
        return force_new_instance();
    }
    let (recorded_entry, mut instance_img, header_index) = result.map_err(|e| {
        error!("Failed to get entry from instance.img: {e}");
        RebootReason::InternalError
    })?;

    let (new_instance, salt) = if let Some(entry) = recorded_entry {
        check_dice_measurements_match_entry(dice_inputs, &entry)?;
        let salt = entry.salt;
        (false, salt)
    } else {
        let salt = random_hidden_input()?;
        let entry = EntryBody::new(dice_inputs, &salt);
        record_instance_entry(&entry, cdi_seal, &mut instance_img, header_index).map_err(|e| {
            error!("Failed to get recorded entry in instance.img: {e}");
            RebootReason::InternalError
        })?;
        (true, salt)
    };

    const HIDDEN_INPUT_MECHANISM_RNG_BASED: &[u8] = b"RNG_BASED";
    let mut hasher = Sha512::new();
    hasher.update(HIDDEN_INPUT_MECHANISM_RNG_BASED);
    hasher.update(&salt);
    let salt_for_dice_hidden_input = hasher.digest();
    Ok((new_instance, salt_for_dice_hidden_input, false))
}

fn force_new_instance() -> Result<(bool, Hidden, bool), RebootReason> {
    info!("No rollback protection mechanism available: generating a new instance");
    Ok((true, random_hidden_input()?, false))
}

fn random_hidden_input() -> Result<Hidden, RebootReason> {
    rand::random_array().map_err(|e| {
        error!("Failed to generate salt: {e}");
        RebootReason::InternalError
    })
}

fn check_dice_measurements_match_entry(
    dice_inputs: &PartialInputs,
    entry: &EntryBody,
) -> Result<(), RebootReason> {
    ensure_dice_measurements_match_entry(dice_inputs, entry).map_err(|e| {
        error!(
            "Dice measurements do not match recorded entry. \
        This may be because of update: {e}"
        );
        RebootReason::InternalError
    })?;

    Ok(())
}

fn ensure_dice_measurements_match_entry(
    dice_inputs: &PartialInputs,
    entry: &EntryBody,
) -> Result<(), InstanceError> {
    if entry.code_hash != dice_inputs.code_hash {
        Err(InstanceError::RecordedCodeHashMismatch)
    } else if entry.auth_hash != dice_inputs.auth_hash {
        Err(InstanceError::RecordedAuthHashMismatch)
    } else if entry.mode() != dice_inputs.mode {
        Err(InstanceError::RecordedDiceModeMismatch)
    } else {
        Ok(())
    }
}

fn should_defer_rollback_protection(fdt: &Fdt) -> Result<bool, RebootReason> {
    let defer_rbp = read_defer_rollback_protection(fdt).map_err(|e| {
        error!("Failed to get defer-rollback-protection property in DT: {e}");
        RebootReason::InvalidFdt
    })?;
    Ok(defer_rbp.is_some())
}
