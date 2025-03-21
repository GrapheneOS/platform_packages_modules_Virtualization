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

mod arch;
mod bootargs;
mod config;
mod device_assignment;
mod dice;
mod entry;
mod fdt;
mod gpt;
mod instance;
mod memory;
mod rollback;

use crate::dice::{DiceChainInfo, PartialInputs};
use crate::entry::RebootReason;
use crate::fdt::{modify_for_next_stage, read_instance_id, sanitize_device_tree};
use crate::rollback::perform_rollback_protection;
use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;
use bssl_avf::Digester;
use diced_open_dice::{
    bcc_handover_parse, DiceArtifacts, DiceContext, Hidden, HIDDEN_SIZE, VM_KEY_ALGORITHM,
};
use libfdt::Fdt;
use log::{debug, error, info, trace, warn};
use pvmfw_avb::verify_payload;
use pvmfw_avb::DebugLevel;
use pvmfw_avb::VerifiedBootData;
use pvmfw_embedded_key::PUBLIC_KEY;
use vmbase::heap;
use vmbase::memory::{flush, SIZE_4KB};
use vmbase::rand;

fn main<'a>(
    untrusted_fdt: &mut Fdt,
    signed_kernel: &[u8],
    ramdisk: Option<&[u8]>,
    current_dice_handover: &[u8],
    mut debug_policy: Option<&[u8]>,
    vm_dtbo: Option<&mut [u8]>,
    vm_ref_dt: Option<&[u8]>,
) -> Result<(Option<&'a [u8]>, bool), RebootReason> {
    info!("pVM firmware");
    debug!("FDT: {:?}", untrusted_fdt.as_ptr());
    debug!("Signed kernel: {:?} ({:#x} bytes)", signed_kernel.as_ptr(), signed_kernel.len());
    debug!("AVB public key: addr={:?}, size={:#x} ({1})", PUBLIC_KEY.as_ptr(), PUBLIC_KEY.len());
    if let Some(rd) = ramdisk {
        debug!("Ramdisk: {:?} ({:#x} bytes)", rd.as_ptr(), rd.len());
    } else {
        debug!("Ramdisk: None");
    }

    let (dice_handover_bytes, dice_cdi_seal, dice_context, dice_debug_mode) =
        parse_dice_handover(current_dice_handover)?;

    // The bootloader should never pass us a debug policy when the boot is secure (the bootloader
    // is locked). If it gets it wrong, disregard it & log it, to avoid it causing problems.
    if debug_policy.is_some() && !dice_debug_mode {
        warn!("Ignoring debug policy, DICE handover does not indicate Debug mode");
        debug_policy = None;
    }

    let (verified_boot_data, debuggable, guest_page_size) = {
        let (dat, debug, sz) = perform_verified_boot(signed_kernel, ramdisk)?;
        (Some(dat), debug, sz)
    };

    let hyp_page_size = hypervisor_backends::get_granule_size();
    let _ =
        sanitize_device_tree(untrusted_fdt, vm_dtbo, vm_ref_dt, guest_page_size, hyp_page_size)?;
    let fdt = untrusted_fdt; // DT has now been sanitized.

    let (next_dice_handover, new_instance) = if let Some(ref data) = verified_boot_data {
        let instance_hash = salt_from_instance_id(fdt)?;
        let dice_inputs = PartialInputs::new(data, instance_hash).map_err(|e| {
            error!("Failed to compute partial DICE inputs: {e:?}");
            RebootReason::InternalError
        })?;
        let (new_instance, salt, defer_rollback_protection) =
            perform_rollback_protection(fdt, data, &dice_inputs, &dice_cdi_seal)?;
        trace!("Got salt for instance: {salt:x?}");

        let next_dice_handover = perform_dice_derivation(
            dice_handover_bytes.as_ref(),
            dice_context,
            dice_inputs,
            &salt,
            defer_rollback_protection,
            guest_page_size,
            guest_page_size,
        )?;

        (Some(next_dice_handover), new_instance)
    } else {
        (None, true)
    };

    let kaslr_seed = u64::from_ne_bytes(rand::random_array().map_err(|e| {
        error!("Failed to generated guest KASLR seed: {e}");
        RebootReason::InternalError
    })?);
    let strict_boot = true;
    modify_for_next_stage(
        fdt,
        next_dice_handover,
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
    Ok((next_dice_handover, debuggable))
}

fn parse_dice_handover(
    bytes: &[u8],
) -> Result<(Cow<'_, [u8]>, Vec<u8>, DiceContext, bool), RebootReason> {
    let dice_handover = bcc_handover_parse(bytes).map_err(|e| {
        error!("Invalid DICE Handover: {e:?}");
        RebootReason::InvalidDiceHandover
    })?;
    trace!("DICE handover: {dice_handover:x?}");

    let dice_chain_info = DiceChainInfo::new(dice_handover.bcc()).map_err(|e| {
        error!("{e}");
        RebootReason::InvalidDiceHandover
    })?;
    let is_debug_mode = dice_chain_info.is_debug_mode();
    let cose_alg = dice_chain_info.leaf_subject_pubkey().cose_alg;
    trace!("DICE chain leaf subject public key algorithm: {:?}", cose_alg);

    let dice_context = DiceContext {
        authority_algorithm: cose_alg.try_into().map_err(|e| {
            error!("{e}");
            RebootReason::InternalError
        })?,
        subject_algorithm: VM_KEY_ALGORITHM,
    };

    let cdi_seal = dice_handover.cdi_seal().to_vec();

    let bytes_for_next = if cfg!(dice_changes) {
        Cow::Borrowed(bytes)
    } else {
        // It is possible that the DICE chain we were given is rooted in the UDS. We do not want to
        // give such a chain to the payload, or even the associated CDIs. So remove the
        // entire chain we were given and taint the CDIs. Note that the resulting CDIs are
        // still deterministically derived from those we received, so will vary iff they do.
        // TODO(b/280405545): Remove this post Android 14.
        let truncated_bytes = dice::chain::truncate(dice_handover).map_err(|e| {
            error!("{e}");
            RebootReason::InternalError
        })?;
        Cow::Owned(truncated_bytes)
    };

    Ok((bytes_for_next, cdi_seal, dice_context, is_debug_mode))
}

fn perform_dice_derivation<'a>(
    dice_handover_bytes: &[u8],
    dice_context: DiceContext,
    dice_inputs: PartialInputs,
    salt: &[u8; HIDDEN_SIZE],
    defer_rollback_protection: bool,
    next_handover_size: usize,
    next_handover_align: usize,
) -> Result<&'a [u8], RebootReason> {
    let next_dice_handover = heap::aligned_boxed_slice(next_handover_size, next_handover_align)
        .ok_or_else(|| {
            error!("Failed to allocate the next-stage DICE handover");
            RebootReason::InternalError
        })?;
    // By leaking the slice, its content will be left behind for the next stage.
    let next_dice_handover = Box::leak(next_dice_handover);

    dice_inputs
        .write_next_handover(
            dice_handover_bytes.as_ref(),
            salt,
            defer_rollback_protection,
            next_dice_handover,
            dice_context,
        )
        .map_err(|e| {
            error!("Failed to derive next-stage DICE secrets: {e:?}");
            RebootReason::SecretDerivationError
        })?;
    flush(next_dice_handover);
    Ok(next_dice_handover)
}

fn perform_verified_boot<'a>(
    signed_kernel: &[u8],
    ramdisk: Option<&[u8]>,
) -> Result<(VerifiedBootData<'a>, bool, usize), RebootReason> {
    let verified_boot_data = verify_payload(signed_kernel, ramdisk, PUBLIC_KEY).map_err(|e| {
        error!("Failed to verify the payload: {e}");
        RebootReason::PayloadVerificationError
    })?;
    let debuggable = verified_boot_data.debug_level != DebugLevel::None;
    if debuggable {
        info!("Successfully verified a debuggable payload.");
        info!("Please disregard any previous libavb ERROR about initrd_normal.");
    }
    let guest_page_size = verified_boot_data.page_size.unwrap_or(SIZE_4KB);

    Ok((verified_boot_data, debuggable, guest_page_size))
}

// Get the "salt" which is one of the input for DICE derivation.
// This provides differentiation of secrets for different VM instances with same payloads.
fn salt_from_instance_id(fdt: &Fdt) -> Result<Option<Hidden>, RebootReason> {
    let Some(id) = read_instance_id(fdt).map_err(|e| {
        error!("Failed to get instance-id in DT: {e}");
        RebootReason::InvalidFdt
    })?
    else {
        return Ok(None);
    };
    let salt = Digester::sha512()
        .digest(&[&b"InstanceId:"[..], id].concat())
        .map_err(|e| {
            error!("Failed to get digest of instance-id: {e}");
            RebootReason::InternalError
        })?
        .try_into()
        .map_err(|_| RebootReason::InternalError)?;
    Ok(Some(salt))
}
