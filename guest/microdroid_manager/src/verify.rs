// Copyright 2023 The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::instance::{ApexData, ApkData, MicrodroidData};
use crate::payload::{get_apex_data_from_payload, to_metadata};
use crate::{is_strict_boot, MicrodroidError};
use anyhow::{anyhow, ensure, Context, Result};
use apkmanifest::get_manifest_info;
use apkverify::{extract_signed_data, verify, V4Signature};
use glob::glob;
use itertools::sorted;
use log::{info, warn};
use microdroid_metadata::{write_metadata, Metadata};
use openssl::sha::sha512;
use rand::Fill;
use rustutils::system_properties;
use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Child, Command};
use std::str;
use std::time::SystemTime;

pub const DM_MOUNTED_APK_PATH: &str = "/dev/block/mapper/microdroid-apk";

const MAIN_APK_PATH: &str = "/dev/block/by-name/microdroid-apk";
const MAIN_APK_IDSIG_PATH: &str = "/dev/block/by-name/microdroid-apk-idsig";
const MAIN_APK_DEVICE_NAME: &str = "microdroid-apk";
const EXTRA_APK_PATH_PATTERN: &str = "/dev/block/by-name/extra-apk-*";
const EXTRA_IDSIG_PATH_PATTERN: &str = "/dev/block/by-name/extra-idsig-*";

const APKDMVERITY_BIN: &str = "/system/bin/apkdmverity";

/// Verify payload before executing it. For APK payload, Full verification (which is slow) is done
/// when the root_hash values from the idsig file and the instance disk are different. This function
/// returns the verified root hash (for APK payload) and pubkeys (for APEX payloads) that can be
/// saved to the instance disk.
pub fn verify_payload(
    metadata: &Metadata,
    saved_data: Option<&MicrodroidData>,
) -> Result<MicrodroidData> {
    let start_time = SystemTime::now();

    // Verify main APK
    let root_hash_from_idsig = get_apk_root_hash_from_idsig(MAIN_APK_IDSIG_PATH)?;
    let root_hash_trustful =
        saved_data.map(|d| d.apk_data.root_hash_eq(root_hash_from_idsig.as_ref())).unwrap_or(false);

    // If root_hash can be trusted, pass it to apkdmverity so that it uses the passed root_hash
    // instead of the value read from the idsig file.
    let main_apk_argument = {
        ApkDmverityArgument {
            apk: MAIN_APK_PATH,
            idsig: MAIN_APK_IDSIG_PATH,
            name: MAIN_APK_DEVICE_NAME,
            saved_root_hash: if root_hash_trustful {
                Some(root_hash_from_idsig.as_ref())
            } else {
                None
            },
        }
    };
    let mut apkdmverity_arguments = vec![main_apk_argument];

    // Verify extra APKs
    // For now, we can't read the payload config, so glob APKs and idsigs.
    // Later, we'll see if it matches with the payload config.

    // sort globbed paths to match apks (extra-apk-{idx}) and idsigs (extra-idsig-{idx})
    // e.g. "extra-apk-0" corresponds to "extra-idsig-0"
    let extra_apks =
        sorted(glob(EXTRA_APK_PATH_PATTERN)?.collect::<Result<Vec<_>, _>>()?).collect::<Vec<_>>();
    let extra_idsigs =
        sorted(glob(EXTRA_IDSIG_PATH_PATTERN)?.collect::<Result<Vec<_>, _>>()?).collect::<Vec<_>>();
    ensure!(
        extra_apks.len() == extra_idsigs.len(),
        "Extra apks/idsigs mismatch: {} apks but {} idsigs",
        extra_apks.len(),
        extra_idsigs.len()
    );

    let extra_root_hashes_from_idsig: Vec<_> = extra_idsigs
        .iter()
        .map(|idsig| {
            get_apk_root_hash_from_idsig(idsig).expect("Can't find root hash from extra idsig")
        })
        .collect();

    let extra_root_hashes_trustful: Vec<_> = if let Some(data) = saved_data {
        extra_root_hashes_from_idsig
            .iter()
            .enumerate()
            .map(|(i, root_hash)| data.extra_apk_root_hash_eq(i, root_hash))
            .collect()
    } else {
        vec![false; extra_root_hashes_from_idsig.len()]
    };
    let extra_apk_names: Vec<_> =
        (0..extra_apks.len()).map(|i| format!("extra-apk-{}", i)).collect();

    for (i, extra_apk) in extra_apks.iter().enumerate() {
        apkdmverity_arguments.push({
            ApkDmverityArgument {
                apk: extra_apk.to_str().unwrap(),
                idsig: extra_idsigs[i].to_str().unwrap(),
                name: &extra_apk_names[i],
                saved_root_hash: if extra_root_hashes_trustful[i] {
                    Some(&extra_root_hashes_from_idsig[i])
                } else {
                    None
                },
            }
        });
    }

    // Start apkdmverity and wait for the dm-verify block
    let mut apkdmverity_child = run_apkdmverity(&apkdmverity_arguments)?;

    // While waiting for apkdmverity to mount APK, gathers public keys and root digests from
    // APEX payload.
    let apex_data_from_payload = get_apex_data_from_payload(metadata)?;

    // To prevent a TOCTOU attack, we need to make sure that when apexd verifies & mounts the
    // APEXes it sees the same ones that we just read - so we write the metadata we just collected
    // to a file (that the host can't access) that apexd will then verify against. See b/199371341.
    write_apex_payload_data(saved_data, &apex_data_from_payload)?;

    if cfg!(not(dice_changes)) {
        // Start apexd to activate APEXes
        system_properties::write("ctl.start", "apexd-vm")?;
    }

    // TODO(inseob): add timeout
    apkdmverity_child.wait()?;

    // Do the full verification if the root_hash is un-trustful. This requires the full scanning of
    // the APK file and therefore can be very slow if the APK is large. Note that this step is
    // taken only when the root_hash is un-trustful which can be either when this is the first boot
    // of the VM or APK was updated in the host.
    // TODO(jooyung): consider multithreading to make this faster

    let main_apk_data =
        get_data_from_apk(DM_MOUNTED_APK_PATH, root_hash_from_idsig, root_hash_trustful)?;

    let extra_apks_data = extra_root_hashes_from_idsig
        .into_iter()
        .enumerate()
        .map(|(i, extra_root_hash)| {
            let mount_path = format!("/dev/block/mapper/{}", &extra_apk_names[i]);
            get_data_from_apk(&mount_path, extra_root_hash, extra_root_hashes_trustful[i])
        })
        .collect::<Result<Vec<_>>>()?;

    info!("payload verification successful. took {:#?}", start_time.elapsed().unwrap());

    // At this point, we can ensure that the root hashes from the idsig files are trusted, either
    // because we have fully verified the APK signature (and apkdmverity checks all the data we
    // verified is consistent with the root hash) or because we have the saved APK data which will
    // be checked as identical to the data we have verified.

    let salt = if cfg!(llpvm_changes) || is_strict_boot() {
        // Salt is obsolete with llpvm_changes.
        vec![0u8; 64]
    } else if let Some(saved_data) = saved_data {
        // Use the salt from a verified instance.
        saved_data.salt.clone()
    } else {
        // Generate a salt for a new instance.
        let mut salt = vec![0u8; 64];
        salt.as_mut_slice().try_fill(&mut rand::thread_rng())?;
        salt
    };

    Ok(MicrodroidData {
        salt,
        apk_data: main_apk_data,
        extra_apks_data,
        apex_data: apex_data_from_payload,
    })
}

fn get_data_from_apk(
    apk_path: &str,
    root_hash: Box<[u8]>,
    root_hash_trustful: bool,
) -> Result<ApkData> {
    let cert_hash = get_cert_hash_from_apk(apk_path, root_hash_trustful)?.to_vec();
    // Read package name etc from the APK manifest. In the unlikely event that they aren't present
    // we use the default values. We simply put these values in the DICE node for the payload, and
    // users of that can decide how to handle blank information - there's no reason for us
    // to fail starting a VM even with such a weird APK.
    let manifest_info = get_manifest_info(apk_path)
        .map_err(|e| warn!("Failed to read manifest info from APK: {e:?}"))
        .unwrap_or_default();

    Ok(ApkData {
        root_hash: root_hash.into(),
        cert_hash,
        package_name: manifest_info.package,
        version_code: manifest_info.version_code,
    })
}

fn write_apex_payload_data(
    saved_data: Option<&MicrodroidData>,
    apex_data_from_payload: &[ApexData],
) -> Result<()> {
    if let Some(saved_apex_data) = saved_data.map(|d| &d.apex_data) {
        // We don't support APEX updates. (assuming that update will change root digest)
        ensure!(
            saved_apex_data == apex_data_from_payload,
            MicrodroidError::PayloadChanged(String::from("APEXes have changed."))
        );
    }
    let apex_metadata = to_metadata(apex_data_from_payload);
    // Pass metadata(with public keys and root digests) to apexd so that it uses the passed
    // metadata instead of the default one (/dev/block/by-name/payload-metadata)
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open("/apex/vm-payload-metadata")
        .context("Failed to open /apex/vm-payload-metadata")
        .and_then(|f| write_metadata(&apex_metadata, f))?;

    Ok(())
}

fn get_apk_root_hash_from_idsig<P: AsRef<Path>>(idsig_path: P) -> Result<Box<[u8]>> {
    Ok(V4Signature::from_idsig_path(idsig_path)?.hashing_info.raw_root_hash)
}

fn get_cert_hash_from_apk(apk: &str, root_hash_trustful: bool) -> Result<[u8; 64]> {
    let current_sdk = get_current_sdk()?;

    let signed_data = if !root_hash_trustful {
        verify(apk, current_sdk).context(MicrodroidError::PayloadVerificationFailed(format!(
            "failed to verify {}",
            apk
        )))
    } else {
        extract_signed_data(apk, current_sdk)
    }?;
    Ok(sha512(signed_data.first_certificate_der()?))
}

fn get_current_sdk() -> Result<u32> {
    let current_sdk = system_properties::read("ro.build.version.sdk")?;
    let current_sdk = current_sdk.ok_or_else(|| anyhow!("SDK version missing"))?;
    current_sdk.parse().context("Malformed SDK version")
}

struct ApkDmverityArgument<'a> {
    apk: &'a str,
    idsig: &'a str,
    name: &'a str,
    saved_root_hash: Option<&'a [u8]>,
}

fn run_apkdmverity(args: &[ApkDmverityArgument]) -> Result<Child> {
    let mut cmd = Command::new(APKDMVERITY_BIN);

    for argument in args {
        cmd.arg("--apk").arg(argument.apk).arg(argument.idsig).arg(argument.name);
        if let Some(root_hash) = argument.saved_root_hash {
            cmd.arg(hex::encode(root_hash));
        } else {
            cmd.arg("none");
        }
    }

    cmd.spawn().context("Spawn apkdmverity")
}
