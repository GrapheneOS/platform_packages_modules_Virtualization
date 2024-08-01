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

//! pVM firmware.

#![no_main]
#![no_std]

extern crate alloc;

mod bcc;
mod bootargs;
mod config;
mod device_assignment;
mod dice;
mod entry;
mod exceptions;
mod fdt;
mod gpt;
mod helpers;
mod instance;
mod memory;

use crate::bcc::Bcc;
use crate::dice::PartialInputs;
use crate::entry::RebootReason;
use crate::fdt::modify_for_next_stage;
use crate::helpers::GUEST_PAGE_SIZE;
use crate::instance::EntryBody;
use crate::instance::Error as InstanceError;
use crate::instance::{get_recorded_entry, record_instance_entry};
use alloc::borrow::Cow;
use alloc::boxed::Box;
use bssl_avf::Digester;
use core::ops::Range;
use cstr::cstr;
use diced_open_dice::{bcc_handover_parse, DiceArtifacts, Hidden};
use fdtpci::{PciError, PciInfo};
use libfdt::{Fdt, FdtNode};
use log::{debug, error, info, trace, warn};
use pvmfw_avb::verify_payload;
use pvmfw_avb::Capability;
use pvmfw_avb::DebugLevel;
use pvmfw_embedded_key::PUBLIC_KEY;
use vmbase::heap;
use vmbase::memory::flush;
use vmbase::memory::MEMORY;
use vmbase::rand;
use vmbase::virtio::pci;

const NEXT_BCC_SIZE: usize = GUEST_PAGE_SIZE;

fn main(
    fdt: &mut Fdt,
    signed_kernel: &[u8],
    ramdisk: Option<&[u8]>,
    current_bcc_handover: &[u8],
    mut debug_policy: Option<&[u8]>,
) -> Result<(Range<usize>, bool), RebootReason> {
    info!("pVM firmware");
    debug!("FDT: {:?}", fdt.as_ptr());
    debug!("Signed kernel: {:?} ({:#x} bytes)", signed_kernel.as_ptr(), signed_kernel.len());
    debug!("AVB public key: addr={:?}, size={:#x} ({1})", PUBLIC_KEY.as_ptr(), PUBLIC_KEY.len());
    if let Some(rd) = ramdisk {
        debug!("Ramdisk: {:?} ({:#x} bytes)", rd.as_ptr(), rd.len());
    } else {
        debug!("Ramdisk: None");
    }

    let bcc_handover = bcc_handover_parse(current_bcc_handover).map_err(|e| {
        error!("Invalid BCC Handover: {e:?}");
        RebootReason::InvalidBcc
    })?;
    trace!("BCC: {bcc_handover:x?}");

    let cdi_seal = bcc_handover.cdi_seal();

    let bcc = Bcc::new(bcc_handover.bcc()).map_err(|e| {
        error!("{e}");
        RebootReason::InvalidBcc
    })?;

    // The bootloader should never pass us a debug policy when the boot is secure (the bootloader
    // is locked). If it gets it wrong, disregard it & log it, to avoid it causing problems.
    if debug_policy.is_some() && !bcc.is_debug_mode() {
        warn!("Ignoring debug policy, BCC does not indicate Debug mode");
        debug_policy = None;
    }

    // Set up PCI bus for VirtIO devices.
    let pci_info = PciInfo::from_fdt(fdt).map_err(handle_pci_error)?;
    debug!("PCI: {:#x?}", pci_info);
    let mut pci_root = pci::initialize(pci_info, MEMORY.lock().as_mut().unwrap()).map_err(|e| {
        error!("Failed to initialize PCI: {e}");
        RebootReason::InternalError
    })?;

    let verified_boot_data = verify_payload(signed_kernel, ramdisk, PUBLIC_KEY).map_err(|e| {
        error!("Failed to verify the payload: {e}");
        RebootReason::PayloadVerificationError
    })?;
    let debuggable = verified_boot_data.debug_level != DebugLevel::None;
    if debuggable {
        info!("Successfully verified a debuggable payload.");
        info!("Please disregard any previous libavb ERROR about initrd_normal.");
    }

    let next_bcc = heap::aligned_boxed_slice(NEXT_BCC_SIZE, GUEST_PAGE_SIZE).ok_or_else(|| {
        error!("Failed to allocate the next-stage BCC");
        RebootReason::InternalError
    })?;
    // By leaking the slice, its content will be left behind for the next stage.
    let next_bcc = Box::leak(next_bcc);

    let dice_inputs = PartialInputs::new(&verified_boot_data).map_err(|e| {
        error!("Failed to compute partial DICE inputs: {e:?}");
        RebootReason::InternalError
    })?;

    let instance_hash = if cfg!(llpvm_changes) { Some(salt_from_instance_id(fdt)?) } else { None };
    let defer_rollback_protection = should_defer_rollback_protection(fdt)?
        && verified_boot_data.has_capability(Capability::SecretkeeperProtection);
    let (new_instance, salt) = if defer_rollback_protection {
        info!("Guest OS is capable of Secretkeeper protection, deferring rollback protection");
        // rollback_index of the image is used as security_version and is expected to be > 0 to
        // discourage implicit allocation.
        if verified_boot_data.rollback_index == 0 {
            error!("Expected positive rollback_index, found 0");
            return Err(RebootReason::InvalidPayload);
        };
        (false, instance_hash.unwrap())
    } else if verified_boot_data.has_capability(Capability::RemoteAttest) {
        info!("Service VM capable of remote attestation detected, performing version checks");
        if service_vm_version::VERSION != verified_boot_data.rollback_index {
            // For RKP VM, we only boot if the version in the AVB footer of its kernel matches
            // the one embedded in pvmfw at build time.
            // This prevents the pvmfw from booting a roll backed RKP VM.
            error!(
                "Service VM version mismatch: expected {}, found {}",
                service_vm_version::VERSION,
                verified_boot_data.rollback_index
            );
            return Err(RebootReason::InvalidPayload);
        }
        (false, instance_hash.unwrap())
    } else {
        info!("Fallback to instance.img based rollback checks");
        let (recorded_entry, mut instance_img, header_index) =
            get_recorded_entry(&mut pci_root, cdi_seal).map_err(|e| {
                error!("Failed to get entry from instance.img: {e}");
                RebootReason::InternalError
            })?;
        let (new_instance, salt) = if let Some(entry) = recorded_entry {
            check_dice_measurements_match_entry(&dice_inputs, &entry)?;
            let salt = instance_hash.unwrap_or(entry.salt);
            (false, salt)
        } else {
            // New instance!
            let salt = instance_hash.map_or_else(rand::random_array, Ok).map_err(|e| {
                error!("Failed to generated instance.img salt: {e}");
                RebootReason::InternalError
            })?;

            let entry = EntryBody::new(&dice_inputs, &salt);
            record_instance_entry(&entry, cdi_seal, &mut instance_img, header_index).map_err(
                |e| {
                    error!("Failed to get recorded entry in instance.img: {e}");
                    RebootReason::InternalError
                },
            )?;
            (true, salt)
        };
        (new_instance, salt)
    };
    trace!("Got salt for instance: {salt:x?}");

    let new_bcc_handover = if cfg!(dice_changes) {
        Cow::Borrowed(current_bcc_handover)
    } else {
        // It is possible that the DICE chain we were given is rooted in the UDS. We do not want to
        // give such a chain to the payload, or even the associated CDIs. So remove the
        // entire chain we were given and taint the CDIs. Note that the resulting CDIs are
        // still deterministically derived from those we received, so will vary iff they do.
        // TODO(b/280405545): Remove this post Android 14.
        let truncated_bcc_handover = bcc::truncate(bcc_handover).map_err(|e| {
            error!("{e}");
            RebootReason::InternalError
        })?;
        Cow::Owned(truncated_bcc_handover)
    };

    dice_inputs
        .write_next_bcc(
            new_bcc_handover.as_ref(),
            &salt,
            instance_hash,
            defer_rollback_protection,
            next_bcc,
        )
        .map_err(|e| {
            error!("Failed to derive next-stage DICE secrets: {e:?}");
            RebootReason::SecretDerivationError
        })?;
    flush(next_bcc);

    let kaslr_seed = u64::from_ne_bytes(rand::random_array().map_err(|e| {
        error!("Failed to generated guest KASLR seed: {e}");
        RebootReason::InternalError
    })?);
    let strict_boot = true;
    modify_for_next_stage(
        fdt,
        next_bcc,
        new_instance,
        strict_boot,
        debug_policy,
        debuggable,
        kaslr_seed,
    )
    .map_err(|e| {
        error!("Failed to configure device tree: {e}");
        RebootReason::InternalError
    })?;

    info!("Starting payload...");

    let bcc_range = {
        let r = next_bcc.as_ptr_range();
        (r.start as usize)..(r.end as usize)
    };

    Ok((bcc_range, debuggable))
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

// Get the "salt" which is one of the input for DICE derivation.
// This provides differentiation of secrets for different VM instances with same payloads.
fn salt_from_instance_id(fdt: &Fdt) -> Result<Hidden, RebootReason> {
    let id = instance_id(fdt)?;
    let salt = Digester::sha512()
        .digest(&[&b"InstanceId:"[..], id].concat())
        .map_err(|e| {
            error!("Failed to get digest of instance-id: {e}");
            RebootReason::InternalError
        })?
        .try_into()
        .map_err(|_| RebootReason::InternalError)?;
    Ok(salt)
}

fn instance_id(fdt: &Fdt) -> Result<&[u8], RebootReason> {
    let node = avf_untrusted_node(fdt)?;
    let id = node.getprop(cstr!("instance-id")).map_err(|e| {
        error!("Failed to get instance-id in DT: {e}");
        RebootReason::InvalidFdt
    })?;
    id.ok_or_else(|| {
        error!("Missing instance-id");
        RebootReason::InvalidFdt
    })
}

fn should_defer_rollback_protection(fdt: &Fdt) -> Result<bool, RebootReason> {
    let node = avf_untrusted_node(fdt)?;
    let defer_rbp = node
        .getprop(cstr!("defer-rollback-protection"))
        .map_err(|e| {
            error!("Failed to get defer-rollback-protection property in DT: {e}");
            RebootReason::InvalidFdt
        })?
        .is_some();
    Ok(defer_rbp)
}

fn avf_untrusted_node(fdt: &Fdt) -> Result<FdtNode, RebootReason> {
    let node = fdt.node(cstr!("/avf/untrusted")).map_err(|e| {
        error!("Failed to get /avf/untrusted node: {e}");
        RebootReason::InvalidFdt
    })?;
    node.ok_or_else(|| {
        error!("/avf/untrusted node is missing in DT");
        RebootReason::InvalidFdt
    })
}

/// Logs the given PCI error and returns the appropriate `RebootReason`.
fn handle_pci_error(e: PciError) -> RebootReason {
    error!("{}", e);
    match e {
        PciError::FdtErrorPci(_)
        | PciError::FdtNoPci
        | PciError::FdtErrorReg(_)
        | PciError::FdtMissingReg
        | PciError::FdtRegEmpty
        | PciError::FdtRegMissingSize
        | PciError::CamWrongSize(_)
        | PciError::FdtErrorRanges(_)
        | PciError::FdtMissingRanges
        | PciError::RangeAddressMismatch { .. }
        | PciError::NoSuitableRange => RebootReason::InvalidFdt,
    }
}
