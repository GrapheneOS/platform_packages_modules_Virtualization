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

use crate::config::reserved_mem::{ResMem, ResMemEntryInfo};
use crate::config::Entries;
use crate::config::TrustedKeysConfig;
use crate::dice::{DiceChainInfo, PartialInputs};
use crate::entry::RebootReason;
use crate::fdt::{modify_for_next_stage, read_instance_id, sanitize_device_tree};
use crate::rollback::perform_rollback_protection;
use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use bssl_crypto::digest::Sha512;
use diced_open_dice::{
    bcc_handover_parse, DiceArtifacts, DiceContext, Hidden, HIDDEN_SIZE, VM_KEY_ALGORITHM,
};
use libfdt::Fdt;
use log::{debug, error, info, trace, warn};
use pvmfw_avb::verify_payload;
use pvmfw_avb::DebugLevel;
use pvmfw_avb::VerifiedBootData;
use vmbase::heap;
use vmbase::memory::SIZE_4KB;
use vmbase::rand;
use zeroize::Zeroize;

/// Trusted public key, used during verification of the signed kernel & ramdisk.
const PUBLIC_KEY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pvmfw_embedded_key_pub.bin"));

fn main<'a>(
    untrusted_fdt: &mut Fdt,
    signed_kernel: &[u8],
    ramdisk: Option<&[u8]>,
    config: &mut Entries,
) -> Result<(&'a [u8], bool), RebootReason> {
    info!("pVM firmware");
    debug!("FDT: {:?}", untrusted_fdt.as_ptr());
    debug!("Signed kernel: {:?} ({:#x} bytes)", signed_kernel.as_ptr(), signed_kernel.len());
    debug!("AVB public key: addr={:?}, size={:#x} ({1})", PUBLIC_KEY.as_ptr(), PUBLIC_KEY.len());
    if let Some(rd) = ramdisk {
        debug!("Ramdisk: {:?} ({:#x} bytes)", rd.as_ptr(), rd.len());
    } else {
        debug!("Ramdisk: None");
    }

    let (parsed_dice, dice_debug_mode) = parse_dice_handover(config.dice_handover.as_deref())?;

    // The bootloader should never pass us a debug policy when the boot is secure (the bootloader
    // is locked). If it gets it wrong, disregard it & log it, to avoid it causing problems.
    let debug_policy = if dice_debug_mode {
        config.debug_policy
    } else if config.debug_policy.is_some() {
        warn!("Ignoring debug policy, DICE handover does not indicate Debug mode");
        None
    } else {
        None
    };

    let trusted_keys_config = if let Some(keys) = config.trusted_keys {
        TrustedKeysConfig::try_from_bytes(keys)
            .map_err(|e| {
                warn!("Failed to parse trusted keys config: {e}");
                warn!("Ignoring trusted keys config");
            })
            .ok()
    } else {
        None
    };
    let extra_keys = trusted_keys_config.as_ref().map(|c| c.trusted_keys()).unwrap_or(&[]);
    let mut trusted_keys = vec![PUBLIC_KEY];
    trusted_keys.extend_from_slice(extra_keys);

    // Policy/Hidden ABI: If the pvmfw loader (typically ABL) didn't pass a DICE handover (which is
    // technically still mandatory, as per the config data specification), skip DICE, AVB, and RBP.
    // This is to support Qualcomm QTVMs, which perform guest image verification in TrustZone.
    let (verified_boot_data, debuggable, guest_page_size) = if parsed_dice.is_none() {
        warn!("Verified boot is disabled!");
        (None, false, SIZE_4KB)
    } else {
        let (dat, debug, sz) = perform_verified_boot(signed_kernel, ramdisk, &trusted_keys)?;
        (Some(dat), debug, sz)
    };

    let hyp_page_size = hypervisor_backends::get_granule_size();
    let fdt = sanitize_device_tree(
        untrusted_fdt,
        config.vm_dtbo.as_deref_mut(),
        config.vm_ref_dt,
        guest_page_size,
        hyp_page_size,
    )?;

    let mut reserved_mem_info: Vec<ResMemEntryInfo> = if let (Some(vb_data), Some(reserved_mem)) =
        (&verified_boot_data, config.reserved_mem.as_deref())
    {
        ResMem::new(reserved_mem, guest_page_size)
            .map_err(|e| {
                error!("Failed to parse reserved memory: {e}");
                RebootReason::InvalidConfig
            })?
            .into_iter()
            .filter(|x| Some(x.vm_name.to_str().unwrap()) == vb_data.name.as_deref())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let next_dice_handover_pages = if let Some((ref handover, _, _)) = parsed_dice {
        // Based on a simple derivation, with generous error margins.
        const EXTRA_DICE_SIZE: usize = 1024;
        let estimated_next_size = handover.len() + EXTRA_DICE_SIZE;

        estimated_next_size.div_ceil(guest_page_size)
    } else {
        0
    };

    // Regions can't share pages, each gets exactly one for simplicity.
    let reserved_mem_pages = reserved_mem_info.len();
    let preserved_memory_pages = reserved_mem_pages + next_dice_handover_pages;
    let preserved_memory = if preserved_memory_pages > 0 {
        let preserved_memory_size = preserved_memory_pages * guest_page_size;
        let slice =
            heap::aligned_boxed_slice(preserved_memory_size, guest_page_size).ok_or_else(|| {
                error!("Failed to allocate the next-stage reserved memory");
                RebootReason::InternalError
            })?;

        // By leaking the slice, its content will be left behind for the next stage.
        Box::leak(slice)
    } else {
        &mut []
    };
    let (preserved_memory_for_resmem, preserved_memory_for_dice) =
        preserved_memory.split_at_mut(reserved_mem_pages * guest_page_size);

    // Move reserved memory to preserved preserved_pages.
    let mut preserved_pages = preserved_memory_for_resmem.chunks_mut(guest_page_size);
    for entry in reserved_mem_info.iter_mut() {
        let page = preserved_pages.next().unwrap();
        let (blob, rest) = page.split_at_mut(entry.blob.len());
        blob.copy_from_slice(entry.blob);
        rest.zeroize();

        entry.blob = blob;
    }

    let (next_dice_handover, new_instance) = if let Some(ref data) = verified_boot_data {
        let instance_hash = salt_from_instance_id(fdt)?;
        let dice_inputs = PartialInputs::new(data, instance_hash).map_err(|e| {
            error!("Failed to compute partial DICE inputs: {e:?}");
            RebootReason::InternalError
        })?;
        let (dice_handover_bytes, dice_cdi_seal, dice_context) =
            parsed_dice.expect("Missing DICE values with VB data");
        let (new_instance, salt, defer_rollback_protection) =
            perform_rollback_protection(fdt, data, &dice_inputs, &dice_cdi_seal)?;
        trace!("Got salt for instance: {salt:x?}");

        perform_dice_derivation(
            dice_handover_bytes.as_ref(),
            dice_context,
            dice_inputs,
            &salt,
            defer_rollback_protection,
            preserved_memory_for_dice,
        )?;

        (Some(preserved_memory_for_dice), new_instance)
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
        next_dice_handover.as_deref(),
        new_instance,
        strict_boot,
        debug_policy,
        debuggable,
        kaslr_seed,
        &reserved_mem_info,
    )
    .map_err(|e| {
        error!("Failed to configure device tree: {e}");
        RebootReason::InternalError
    })?;

    info!("Starting payload...");

    Ok((preserved_memory, debuggable))
}

fn parse_dice_handover(
    bytes: Option<&[u8]>,
) -> Result<(Option<(Cow<'_, [u8]>, Vec<u8>, DiceContext)>, bool), RebootReason> {
    let Some(bytes) = bytes else {
        return Ok((None, false));
    };
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
    trace!("DICE chain leaf subject public key algorithm: {cose_alg:?}");

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

    Ok((Some((bytes_for_next, cdi_seal, dice_context)), is_debug_mode))
}

fn perform_dice_derivation(
    dice_handover_bytes: &[u8],
    dice_context: DiceContext,
    dice_inputs: PartialInputs,
    salt: &[u8; HIDDEN_SIZE],
    defer_rollback_protection: bool,
    next_dice_handover: &mut [u8],
) -> Result<(), RebootReason> {
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
    Ok(())
}

fn perform_verified_boot<'a>(
    signed_kernel: &[u8],
    ramdisk: Option<&[u8]>,
    trusted_keys: &'a [&'a [u8]],
) -> Result<(VerifiedBootData<'a>, bool, usize), RebootReason> {
    let verified_boot_data = verify_payload(signed_kernel, ramdisk, trusted_keys).map_err(|e| {
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
    let mut ctx = Sha512::new();
    ctx.update(b"InstanceId:");
    ctx.update(id);
    Ok(Some(ctx.digest()))
}
