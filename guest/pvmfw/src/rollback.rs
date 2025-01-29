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

use crate::dice::PartialInputs;
use crate::entry::RebootReason;
use crate::fdt::read_defer_rollback_protection;
use crate::instance::EntryBody;
use crate::instance::Error as InstanceError;
use crate::instance::{get_recorded_entry, record_instance_entry};
use diced_open_dice::Hidden;
use libfdt::Fdt;
use log::{error, info};
use pvmfw_avb::Capability;
use pvmfw_avb::VerifiedBootData;
use virtio_drivers::transport::pci::bus::{ConfigurationAccess, PciRoot};
use vmbase::fdt::{pci::PciInfo, SwiotlbInfo};
use vmbase::memory::init_shared_pool;
use vmbase::rand;
use vmbase::virtio::pci;

/// Performs RBP based on the input payload, current DICE chain, and host-controlled platform.
///
/// On success, returns a tuple containing:
/// - `new_instance`: true if the legacy instance.img solution was used and a new entry created;
/// - `salt`: the salt representing the instance, to be used during DICE derivation;
/// - `defer_rollback_protection`: if RBP is being deferred.
pub fn perform_rollback_protection(
    fdt: &Fdt,
    verified_boot_data: &VerifiedBootData,
    dice_inputs: &PartialInputs,
    cdi_seal: &[u8],
    instance_hash: Option<Hidden>,
) -> Result<(bool, Hidden, bool), RebootReason> {
    if let Some(fixed) = get_fixed_rollback_protection(verified_boot_data) {
        // Prevent attackers from impersonating well-known images.
        perform_fixed_index_rollback_protection(verified_boot_data, fixed)?;
        Ok((false, instance_hash.unwrap(), false))
    } else if (should_defer_rollback_protection(fdt)?
        && verified_boot_data.has_capability(Capability::SecretkeeperProtection))
        || verified_boot_data.has_capability(Capability::TrustySecurityVm)
    {
        perform_deferred_rollback_protection(verified_boot_data)?;
        Ok((false, instance_hash.unwrap(), true))
    } else {
        perform_legacy_rollback_protection(fdt, dice_inputs, cdi_seal, instance_hash)
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

fn get_fixed_rollback_protection(verified_boot_data: &VerifiedBootData) -> Option<u64> {
    if verified_boot_data.has_capability(Capability::RemoteAttest) {
        Some(service_vm_version::VERSION)
    } else {
        None
    }
}

fn perform_fixed_index_rollback_protection(
    verified_boot_data: &VerifiedBootData,
    fixed_index: u64,
) -> Result<(), RebootReason> {
    info!("Performing fixed-index rollback protection");
    let index = verified_boot_data.rollback_index;
    if index != fixed_index {
        error!("Rollback index mismatch: expected {fixed_index}, found {index}");
        Err(RebootReason::InvalidPayload)
    } else {
        Ok(())
    }
}

/// Performs RBP using instance.img where updates require clearing old entries, causing new CDIs.
fn perform_legacy_rollback_protection(
    fdt: &Fdt,
    dice_inputs: &PartialInputs,
    cdi_seal: &[u8],
    instance_hash: Option<Hidden>,
) -> Result<(bool, Hidden, bool), RebootReason> {
    info!("Fallback to instance.img based rollback checks");
    let mut pci_root = initialize_instance_img_device(fdt)?;
    let (recorded_entry, mut instance_img, header_index) =
        get_recorded_entry(&mut pci_root, cdi_seal).map_err(|e| {
            error!("Failed to get entry from instance.img: {e}");
            RebootReason::InternalError
        })?;
    let (new_instance, salt) = if let Some(entry) = recorded_entry {
        check_dice_measurements_match_entry(dice_inputs, &entry)?;
        let salt = instance_hash.unwrap_or(entry.salt);
        (false, salt)
    } else {
        // New instance!
        let salt = instance_hash.map_or_else(rand::random_array, Ok).map_err(|e| {
            error!("Failed to generated instance.img salt: {e}");
            RebootReason::InternalError
        })?;

        let entry = EntryBody::new(dice_inputs, &salt);
        record_instance_entry(&entry, cdi_seal, &mut instance_img, header_index).map_err(|e| {
            error!("Failed to get recorded entry in instance.img: {e}");
            RebootReason::InternalError
        })?;
        (true, salt)
    };
    Ok((new_instance, salt, false))
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

/// Set up PCI bus and VirtIO-blk device containing the instance.img partition.
fn initialize_instance_img_device(
    fdt: &Fdt,
) -> Result<PciRoot<impl ConfigurationAccess>, RebootReason> {
    let pci_info = PciInfo::from_fdt(fdt).map_err(|e| {
        error!("Failed to detect PCI from DT: {e}");
        RebootReason::InvalidFdt
    })?;
    let swiotlb_range = SwiotlbInfo::new_from_fdt(fdt)
        .map_err(|e| {
            error!("Failed to detect swiotlb from DT: {e}");
            RebootReason::InvalidFdt
        })?
        .and_then(|info| info.fixed_range());

    let pci_root = pci::initialize(pci_info).map_err(|e| {
        error!("Failed to initialize PCI: {e}");
        RebootReason::InternalError
    })?;
    init_shared_pool(swiotlb_range).map_err(|e| {
        error!("Failed to initialize shared pool: {e}");
        RebootReason::InternalError
    })?;

    Ok(pci_root)
}
