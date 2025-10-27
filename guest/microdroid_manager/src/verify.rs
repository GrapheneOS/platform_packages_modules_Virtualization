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

use crate::instance::{ApexData, ApkData, EncryptedStoreMode, MicrodroidData};
use crate::payload::{get_apex_data_from_payload, get_tenant_apex_data_from_payload, to_metadata};
use crate::MicrodroidError;
use anyhow::{anyhow, bail, ensure, Context, Result};
use apkmanifest::{get_manifest_info, ApkManifestInfo};
use apkverify::{extract_signed_data, verify, V4Signature};
use glob::glob;
use itertools::sorted;
use log::{info, warn};
use microdroid_metadata::{write_metadata, Metadata};
use microdroid_payload_config::TenantConfig;
use microdroid_payload_config::TenantConfiguration;
use openssl::sha::sha512;
use rustutils::android::system_properties;
use std::collections::HashMap;
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
const TENANT_APK_PATH_PATTERN: &str = "/dev/block/by-name/tenant-apk-*";
const TENANT_IDSIG_PATH_PATTERN: &str = "/dev/block/by-name/tenant-idsig-*";

const APKDMVERITY_BIN: &str = "/system/bin/apkdmverity";

/// Verify payload before executing it. For APK payload, Full verification (which is slow) is done
/// when the root_hash values from the idsig file and the instance disk are different. This function
/// returns the verified root hash (for APK payload) and pubkeys (for APEX payloads) that can be
/// saved to the instance disk.
pub fn verify_payload(
    metadata: &Metadata,
    saved_data: Option<&MicrodroidData>,
) -> Result<(MicrodroidData, Vec<ApexData>)> {
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
    let extra_apk_names: Vec<_> = (0..extra_apks.len()).map(|i| format!("extra-apk-{i}")).collect();

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

    let tenant_apex_data_from_payload = get_tenant_apex_data_from_payload(metadata)?;

    // To prevent a TOCTOU attack, we need to make sure that when apexd verifies & mounts the
    // APEXes it sees the same ones that we just read - so we write the metadata we just collected
    // to a file (that the host can't access) that apexd will then verify against. See b/199371341.
    if let Some(saved) = saved_data {
        // We don't support APEX updates. (assuming that update will change root digest)
        ensure!(
            saved.apex_data == apex_data_from_payload,
            MicrodroidError::PayloadChanged(String::from(
                "APEXes have changed, have you considered
                including apex as an updatable Tenant?"
            ))
        );
    }
    let mut all_apex_data = Vec::new();
    all_apex_data.extend_from_slice(&apex_data_from_payload);
    all_apex_data.extend_from_slice(&tenant_apex_data_from_payload);

    // Pass metadata(with public keys and root digests) to apexd so that it uses the passed
    // metadata instead of the default one (/dev/block/by-name/payload-metadata)
    write_apex_payload_data(&all_apex_data)?;

    if cfg!(not(dice_changes)) {
        // Start apexd to activate APEXes
        system_properties::write("ctl.start", "apexd-vm")?;
    }

    let exitcode = apkdmverity_child.wait()?;
    ensure!(exitcode.success(), "apkdmverity failed with {:?}", exitcode);

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
    Ok((
        MicrodroidData {
            apk_data: main_apk_data,
            extra_apks_data,
            apex_data: apex_data_from_payload,
        },
        tenant_apex_data_from_payload,
    ))
}

pub(crate) fn integrity_protect_tenant_apks() -> Result<Vec<ApkData>> {
    // sort globbed paths to match apks (tenant-{idx}) and idsigs (tenant-{idx})
    // e.g. "tenant-0" corresponds to "tenant-idsig-0"
    let tenant_apks =
        sorted(glob(TENANT_APK_PATH_PATTERN)?.collect::<Result<Vec<_>, _>>()?).collect::<Vec<_>>();
    let tenant_idsigs = sorted(glob(TENANT_IDSIG_PATH_PATTERN)?.collect::<Result<Vec<_>, _>>()?)
        .collect::<Vec<_>>();
    ensure!(
        tenant_apks.len() == tenant_idsigs.len(),
        "Tenant apks/idsigs mismatch: {} apks but {} idsigs",
        tenant_apks.len(),
        tenant_idsigs.len()
    );
    if tenant_apks.is_empty() {
        return Ok(vec![]);
    }
    let tenant_hashes_from_idsig: Vec<_> = tenant_idsigs
        .iter()
        .map(|idsig| {
            get_apk_root_hash_from_idsig(idsig).expect("Can't find root hash from tenant idsig")
        })
        .collect();

    let tenant_apk_block_dev: Vec<_> =
        (0..tenant_apks.len()).map(|i| format!("tenant-apk-{}", i)).collect();
    let mut apkdmverity_arguments: Vec<ApkDmverityArgument> = vec![];
    for (i, tenant_apk) in tenant_apks.iter().enumerate() {
        apkdmverity_arguments.push({
            ApkDmverityArgument {
                apk: tenant_apk.to_str().unwrap(),
                idsig: tenant_idsigs[i].to_str().unwrap(),
                name: &tenant_apk_block_dev[i],
                saved_root_hash: None,
            }
        });
    }
    // Start apkdmverity and wait for the dm-verify block
    let mut apkdmverity_child = run_apkdmverity(&apkdmverity_arguments)?;

    let exitcode = apkdmverity_child.wait()?;
    ensure!(exitcode.success(), "apkdmverity failed with {:?}", exitcode);

    let tenant_apks_data = tenant_hashes_from_idsig
        .into_iter()
        .enumerate()
        .map(|(i, root_hash)| {
            let mount_path = format!("/dev/block/mapper/{}", &tenant_apk_block_dev[i]);
            get_data_from_apk(&mount_path, root_hash, false)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(tenant_apks_data)
}

// Validation logic includes:
// 1. The tenant_apk exactly matches apks described in tenant_config (comparison is by package name)
// 2. The order of description in tenant_config is irrelevant.
// 3. The rollback_index (or version_code if rollback_index is missing) >=  min_version in
//    tenant_config
// 4. The cert_hash of tenant apk == expected_authority in tenant_config
pub(crate) fn validate_tenant_apks_against_tenant_config(
    tenant_apk: &[ApkData], // data extracted from the apk passed from host
    tenant_config: &[TenantConfig],
) -> Result<()> {
    let apk_configs: Vec<&TenantConfiguration> = tenant_config
        .iter()
        .filter_map(
            |config| {
                if let TenantConfig::Apk(config) = config {
                    Some(config)
                } else {
                    None
                }
            },
        )
        .collect();
    let config_map: HashMap<&String, &TenantConfiguration> =
        apk_configs.iter().map(|&c| (&c.name, c)).collect();

    let apk_map: HashMap<&String, &ApkData> =
        tenant_apk.iter().map(|apk| (&apk.package_name, apk)).collect();

    // Since the following loop verifies that every provided APK is defined in the configuration,
    // this length check is sufficient to guarantee that the set of provided APKs is exactly what
    // the configuration specifies.
    if apk_map.len() != config_map.len() {
        bail!(MicrodroidError::PayloadVerificationFailed(
            "Provided tenant APKs do not match the configuration".to_string()
        ));
    }

    for (package_name, apk_data) in &apk_map {
        // This unwrap is safe because we've checked that the key sets of both maps are identical.
        let config = config_map.get(*package_name).unwrap();
        // Version check!
        if let Some(min_version) = config.min_version {
            // Check rollback_index (or version_code if rollback_index is missing)  against
            // min_version
            let version = apk_data.rollback_index.map_or(apk_data.version_code, u64::from);
            if version < min_version {
                bail!(MicrodroidError::PayloadVerificationFailed(format!(
                    "APK ('{}') version ({}) is less than min_version ({})",
                    package_name, version, min_version
                )));
            }
        }
        // Expected authority check!
        if let Some(expected_auth) = &config.expected_authority {
            // Check version_code against min_version
            let cert_hash = hex::encode(&apk_data.cert_hash);
            if *expected_auth != cert_hash {
                bail!(MicrodroidError::PayloadVerificationFailed(format!(
                    "APK ('{}') cert_hash ('{}') mismatches expected authority ({})",
                    package_name, cert_hash, expected_auth
                )));
            }
        }
    }

    Ok(())
}

fn validate_manifest_info(info: &ApkManifestInfo) -> Result<()> {
    ensure!(
        info.has_relaxed_rollback_protection_permission == info.rollback_index.is_some(),
        MicrodroidError::PayloadVerificationFailed(String::from("to opt in relaxed rollback protection scheme manifest must request android.permission.USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION permission and set the android.system.virtualmachine.ROLLBACK_INDEX property"))
    );
    Ok(())
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

    validate_manifest_info(&manifest_info)?;

    Ok(ApkData {
        root_hash: root_hash.into(),
        cert_hash,
        package_name: manifest_info.package,
        version_code: manifest_info.version_code,
        rollback_index: manifest_info.rollback_index,
        encrypted_store_mode: EncryptedStoreMode::from(manifest_info.encrypted_store_mode),
    })
}

fn write_apex_payload_data(data: &[ApexData]) -> Result<()> {
    let apex_metadata = to_metadata(data);
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
        verify(apk, current_sdk)
            .context(MicrodroidError::PayloadVerificationFailed(format!("failed to verify {apk}")))
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
