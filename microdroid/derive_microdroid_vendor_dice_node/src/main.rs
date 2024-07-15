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

//! Derives microdroid vendor dice node.

use anyhow::{bail, Context, Result};
use clap::Parser;
use cstr::cstr;
use dice_driver::DiceDriver;
use diced_open_dice::{
    hash, retry_bcc_format_config_descriptor, DiceConfigValues, OwnedDiceArtifacts, HIDDEN_SIZE,
};
use dm::util::blkgetsize64;
use std::fs::{read_link, File};
use std::path::{Path, PathBuf};
use vbmeta::VbMetaImage;

const AVF_STRICT_BOOT: &str = "/proc/device-tree/chosen/avf,strict-boot";

#[derive(Parser)]
struct Args {
    /// Path to the dice driver (e.g. /dev/open-dice0)
    #[arg(long)]
    dice_driver: PathBuf,
    /// Path to the microdroid-vendor.img disk image.
    #[arg(long)]
    microdroid_vendor_disk_image: PathBuf,
    /// File to save resulting dice chain to.
    #[arg(long)]
    output: PathBuf,
}

// TODO(ioffe): move to a library to reuse same code here, in microdroid_manager and in
// first_stage_init.
fn is_strict_boot() -> bool {
    Path::new(AVF_STRICT_BOOT).exists()
}

// See dice_for_avf_guest.cddl for CDDL of Configuration Descriptor of VM components.
fn build_descriptor(vbmeta: &VbMetaImage) -> Result<Vec<u8>> {
    let values = DiceConfigValues {
        component_name: Some(cstr!("Microdroid vendor")),
        security_version: Some(vbmeta.rollback_index()),
        ..Default::default()
    };
    Ok(retry_bcc_format_config_descriptor(&values)?)
}

// TODO(ioffe): move to libvbmeta.
fn find_root_digest(vbmeta: &VbMetaImage) -> Result<Option<Vec<u8>>> {
    for descriptor in vbmeta.descriptors()?.iter() {
        if let vbmeta::Descriptor::Hashtree(_) = descriptor {
            return Ok(Some(descriptor.to_hashtree()?.root_digest().to_vec()));
        }
    }
    Ok(None)
}

fn dice_derivation(dice: DiceDriver, vbmeta: &VbMetaImage) -> Result<OwnedDiceArtifacts> {
    let authority_hash = if let Some(pubkey) = vbmeta.public_key() {
        hash(pubkey).context("hash pubkey")?
    } else {
        bail!("no public key")
    };
    let code_hash = if let Some(root_digest) = find_root_digest(vbmeta)? {
        hash(root_digest.as_ref()).context("hash root_digest")?
    } else {
        bail!("no hashtree")
    };
    let desc = build_descriptor(vbmeta).context("build descriptor")?;
    let hidden = [0; HIDDEN_SIZE];
    // The microdroid vendor partition doesn't contribute to the debuggability of the VM, and it is
    // a bit tricky to propagate the info on whether the VM is debuggable to
    // derive_microdroid_dice_node binary. Given these, we just always set `is_debuggable: false`
    // for the "Microdroid vendor" dice node. The adjacent dice nodes (pvmfw & payload) provide the
    // accurate information on whether VM is debuggable.
    dice.derive(code_hash, &desc, authority_hash, /* debug= */ false, hidden)
}

fn extract_vbmeta(block_dev: &Path) -> Result<VbMetaImage> {
    let size = blkgetsize64(block_dev).context("blkgetsize64  failed")?;
    let file = File::open(block_dev).context("open failed")?;
    let vbmeta = VbMetaImage::verify_reader_region(file, 0, size)?;
    Ok(vbmeta)
}

fn try_main() -> Result<()> {
    let args = Args::parse();
    let dice =
        DiceDriver::new(&args.dice_driver, is_strict_boot()).context("Failed to load DICE")?;
    let path = read_link(args.microdroid_vendor_disk_image).context("failed to read symlink")?;
    let vbmeta = extract_vbmeta(&path).context("failed to extract vbmeta")?;
    let dice_artifacts = dice_derivation(dice, &vbmeta).context("failed to derive dice chain")?;
    let file = File::create(&args.output).context("failed to create output")?;
    serde_cbor::to_writer(file, &dice_artifacts).context("failed to write dice artifacts")?;
    Ok(())
}

fn main() {
    if let Err(e) = try_main() {
        eprintln!("failed with {:?}", e);
        std::process::exit(1);
    }
}
