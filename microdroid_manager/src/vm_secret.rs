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

//! Class for encapsulating & managing represent VM secrets.

use anyhow::{anyhow, ensure, Context, Result};
use android_system_virtualmachineservice::aidl::android::system::virtualmachineservice::IVirtualMachineService::IVirtualMachineService;
use android_hardware_security_secretkeeper::aidl::android::hardware::security::secretkeeper::ISecretkeeper::ISecretkeeper;
use secretkeeper_comm::data_types::request::Request;
use binder::{Strong};
use coset::{CoseKey, CborSerializable, CborOrdering};
use dice_policy_builder::{TargetEntry, ConstraintSpec, ConstraintType, policy_for_dice_chain, MissingAction, WILDCARD_FULL_ARRAY};
use diced_open_dice::{DiceArtifacts, OwnedDiceArtifacts};
use keystore2_crypto::ZVec;
use openssl::hkdf::hkdf;
use openssl::md::Md;
use openssl::sha;
use secretkeeper_client::dice::OwnedDiceArtifactsWithExplicitKey;
use secretkeeper_client::SkSession;
use secretkeeper_comm::data_types::{Id, ID_SIZE, Secret, SECRET_SIZE};
use secretkeeper_comm::data_types::response::Response;
use secretkeeper_comm::data_types::packet::{ResponsePacket, ResponseType};
use secretkeeper_comm::data_types::request_response_impl::{
    StoreSecretRequest, GetSecretResponse, GetSecretRequest};
use secretkeeper_comm::data_types::error::SecretkeeperError;
use std::fs;
use zeroize::Zeroizing;

const ENCRYPTEDSTORE_KEY_IDENTIFIER: &str = "encryptedstore_key";
const AUTHORITY_HASH: i64 = -4670549;
const MODE: i64 = -4670551;
const CONFIG_DESC: i64 = -4670548;
const SECURITY_VERSION: i64 = -70005;
const SUBCOMPONENT_DESCRIPTORS: i64 = -71002;
const SUBCOMPONENT_SECURITY_VERSION: i64 = 2;
const SUBCOMPONENT_AUTHORITY_HASH: i64 = 4;
// See dice_for_avf_guest.cddl for the `component_name` used by different boot stages in guest VM.
const MICRODROID_PAYLOAD_COMPONENT_NAME: &str = "Microdroid payload";
const GUEST_OS_COMPONENT_NAME: &str = "vm_entry";
const INSTANCE_HASH_KEY: i64 = -71003;

// Generated using hexdump -vn32 -e'14/1 "0x%02X, " 1 "\n"' /dev/urandom
const SALT_ENCRYPTED_STORE: &[u8] = &[
    0xFC, 0x1D, 0x35, 0x7B, 0x96, 0xF3, 0xEF, 0x17, 0x78, 0x7D, 0x70, 0xED, 0xEA, 0xFE, 0x1D, 0x6F,
    0xB3, 0xF9, 0x40, 0xCE, 0xDD, 0x99, 0x40, 0xAA, 0xA7, 0x0E, 0x92, 0x73, 0x90, 0x86, 0x4A, 0x75,
];
const SALT_PAYLOAD_SERVICE: &[u8] = &[
    0x8B, 0x0F, 0xF0, 0xD3, 0xB1, 0x69, 0x2B, 0x95, 0x84, 0x2C, 0x9E, 0x3C, 0x99, 0x56, 0x7A, 0x22,
    0x55, 0xF8, 0x08, 0x23, 0x81, 0x5F, 0xF5, 0x16, 0x20, 0x3E, 0xBE, 0xBA, 0xB7, 0xA8, 0x43, 0x92,
];

pub enum VmSecret {
    // V2 secrets are derived from 2 independently secured secrets:
    //      1. Secretkeeper protected secrets (skp secret).
    //      2. Dice Sealing CDIs (Similar to V1).
    //
    // These are protected against rollback of boot images i.e. VM instance rebooted
    // with downgraded images will not have access to VM's secret.
    // V2 secrets require hardware support - Secretkeeper HAL, which (among other things)
    // is backed by tamper-evident storage, providing rollback protection to these secrets.
    V2 { dice_artifacts: OwnedDiceArtifactsWithExplicitKey, skp_secret: ZVec },
    // V1 secrets are not protected against rollback of boot images.
    // They are reliable only if rollback of images was prevented by verified boot ie,
    // each stage (including pvmfw/Microdroid/Microdroid Manager) prevents downgrade of next
    // stage. These are now legacy secrets & used only when Secretkeeper HAL is not supported
    // by device.
    V1 { dice_artifacts: OwnedDiceArtifacts },
}

// For supporting V2 secrets, guest expects the public key to be present in the Linux device tree.
fn get_secretkeeper_identity() -> Result<CoseKey> {
    let key = fs::read(super::SECRETKEEPER_KEY)?;
    let mut key = CoseKey::from_slice(&key)?;
    key.canonicalize(CborOrdering::Lexicographic);
    Ok(key)
}

impl VmSecret {
    pub fn new(
        dice_artifacts: OwnedDiceArtifacts,
        vm_service: &Strong<dyn IVirtualMachineService>,
    ) -> Result<Self> {
        ensure!(dice_artifacts.bcc().is_some(), "Dice chain missing");
        if !crate::should_defer_rollback_protection() {
            return Ok(Self::V1 { dice_artifacts });
        }

        let explicit_dice = OwnedDiceArtifactsWithExplicitKey::from_owned_artifacts(dice_artifacts)
            .context("Failed to get Dice artifacts in explicit key format")?;
        // For pVM, skp_secret are stored in Secretkeeper. For non-protected it is all 0s.
        let mut skp_secret = Zeroizing::new([0u8; SECRET_SIZE]);
        if super::is_strict_boot() {
            let sk_service = get_secretkeeper_service(vm_service)?;
            let mut session =
                SkSession::new(sk_service, &explicit_dice, Some(get_secretkeeper_identity()?))?;
            let id = super::get_instance_id()?.ok_or(anyhow!("Missing instance_id"))?;
            let explicit_dice_chain = explicit_dice
                .explicit_key_dice_chain()
                .ok_or(anyhow!("Missing explicit dice chain, this is unusual"))?;
            let policy = sealing_policy(explicit_dice_chain)
                .map_err(|e| anyhow!("Failed to build a sealing_policy: {e}"))?;
            if let Some(secret) = get_secret(&mut session, id, Some(policy.clone()))? {
                *skp_secret = secret;
            } else {
                log::warn!(
                    "No entry found in Secretkeeper for this VM instance, creating new secret."
                );
                *skp_secret = rand::random();
                store_secret(&mut session, id, skp_secret.clone(), policy)?;
            }
        }
        Ok(Self::V2 {
            dice_artifacts: explicit_dice,
            skp_secret: ZVec::try_from(skp_secret.to_vec())?,
        })
    }

    pub fn dice_artifacts(&self) -> &dyn DiceArtifacts {
        match self {
            Self::V2 { dice_artifacts, .. } => dice_artifacts,
            Self::V1 { dice_artifacts } => dice_artifacts,
        }
    }

    fn get_vm_secret(&self, salt: &[u8], identifier: &[u8], key: &mut [u8]) -> Result<()> {
        match self {
            Self::V2 { dice_artifacts, skp_secret } => {
                let mut hasher = sha::Sha256::new();
                hasher.update(dice_artifacts.cdi_seal());
                hasher.update(skp_secret);
                hkdf(key, Md::sha256(), &hasher.finish(), salt, identifier)?
            }
            Self::V1 { dice_artifacts } => {
                hkdf(key, Md::sha256(), dice_artifacts.cdi_seal(), salt, identifier)?
            }
        }
        Ok(())
    }

    /// Derive sealing key for payload with following identifier.
    pub fn derive_payload_sealing_key(&self, identifier: &[u8], key: &mut [u8]) -> Result<()> {
        self.get_vm_secret(SALT_PAYLOAD_SERVICE, identifier, key)
    }

    /// Derive encryptedstore key. This uses hardcoded random salt & fixed identifier.
    pub fn derive_encryptedstore_key(&self, key: &mut [u8]) -> Result<()> {
        self.get_vm_secret(SALT_ENCRYPTED_STORE, ENCRYPTEDSTORE_KEY_IDENTIFIER.as_bytes(), key)
    }
}

// Construct a sealing policy on the dice chain. VMs uses the following set of constraint for
// protecting secrets against rollback of boot images.
// 1. ExactMatch on AUTHORITY_HASH (Required ie, each DiceChainEntry must have it).
// 2. ExactMatch on MODE (Required) - Secret should be inaccessible if any of the runtime
//    configuration changes. For ex, the secrets stored with a boot stage being in Normal mode
//    should be inaccessible when the same stage is booted in Debug mode.
// 3. GreaterOrEqual on SECURITY_VERSION (Optional): The secrets will be accessible if version of
//    any image is greater or equal to the set version. This is an optional field, certain
//    components may chose to prevent booting of rollback images for ex, ABL is expected to provide
//    rollback protection of pvmfw. Such components may chose to not put SECURITY_VERSION in the
//    corresponding DiceChainEntry.
//  4. For each Subcomponent on the last DiceChainEntry (which corresponds to VM payload, See
//     dice_for_avf_guest.cddl):
//       - GreaterOrEqual on SECURITY_VERSION (Required)
//       - ExactMatch on AUTHORITY_HASH (Required).
//  5. ExactMatch on Instance Hash (Required) - This uniquely identifies one VM instance from
//     another even if they are running the exact same images.
fn sealing_policy(dice: &[u8]) -> Result<Vec<u8>, String> {
    let constraint_spec = vec![
        ConstraintSpec::new(
            ConstraintType::ExactMatch,
            vec![AUTHORITY_HASH],
            MissingAction::Fail,
            TargetEntry::All,
        ),
        ConstraintSpec::new(
            ConstraintType::ExactMatch,
            vec![MODE],
            MissingAction::Fail,
            TargetEntry::All,
        ),
        ConstraintSpec::new(
            ConstraintType::GreaterOrEqual,
            vec![CONFIG_DESC, SECURITY_VERSION],
            MissingAction::Ignore,
            TargetEntry::All,
        ),
        ConstraintSpec::new(
            ConstraintType::GreaterOrEqual,
            vec![
                CONFIG_DESC,
                SUBCOMPONENT_DESCRIPTORS,
                WILDCARD_FULL_ARRAY,
                SUBCOMPONENT_SECURITY_VERSION,
            ],
            MissingAction::Fail,
            TargetEntry::ByName(MICRODROID_PAYLOAD_COMPONENT_NAME.to_string()),
        ),
        ConstraintSpec::new(
            ConstraintType::ExactMatch,
            vec![
                CONFIG_DESC,
                SUBCOMPONENT_DESCRIPTORS,
                WILDCARD_FULL_ARRAY,
                SUBCOMPONENT_AUTHORITY_HASH,
            ],
            MissingAction::Fail,
            TargetEntry::ByName(MICRODROID_PAYLOAD_COMPONENT_NAME.to_string()),
        ),
        ConstraintSpec::new(
            ConstraintType::ExactMatch,
            vec![CONFIG_DESC, INSTANCE_HASH_KEY],
            MissingAction::Fail,
            TargetEntry::ByName(GUEST_OS_COMPONENT_NAME.to_string()),
        ),
    ];

    policy_for_dice_chain(dice, constraint_spec)?
        .to_vec()
        .map_err(|e| format!("DicePolicy construction failed {e:?}"))
}

fn store_secret(
    session: &mut SkSession,
    id: [u8; ID_SIZE],
    secret: Zeroizing<[u8; SECRET_SIZE]>,
    sealing_policy: Vec<u8>,
) -> Result<()> {
    let store_request = StoreSecretRequest { id: Id(id), secret: Secret(*secret), sealing_policy };
    log::info!("Secretkeeper operation: {:?}", store_request);

    let store_request = store_request.serialize_to_packet().to_vec().map_err(anyhow_err)?;
    let store_response = session.secret_management_request(&store_request)?;
    let store_response = ResponsePacket::from_slice(&store_response).map_err(anyhow_err)?;
    let response_type = store_response.response_type().map_err(anyhow_err)?;
    ensure!(
        response_type == ResponseType::Success,
        "Secretkeeper store failed with error: {:?}",
        *SecretkeeperError::deserialize_from_packet(store_response).map_err(anyhow_err)?
    );
    Ok(())
}

fn get_secret(
    session: &mut SkSession,
    id: [u8; ID_SIZE],
    updated_sealing_policy: Option<Vec<u8>>,
) -> Result<Option<[u8; SECRET_SIZE]>> {
    let get_request = GetSecretRequest { id: Id(id), updated_sealing_policy };
    log::info!("Secretkeeper operation: {:?}", get_request);
    let get_request = get_request.serialize_to_packet().to_vec().map_err(anyhow_err)?;
    let get_response = session.secret_management_request(&get_request)?;
    let get_response = ResponsePacket::from_slice(&get_response).map_err(anyhow_err)?;
    let response_type = get_response.response_type().map_err(anyhow_err)?;
    if response_type == ResponseType::Success {
        let get_response =
            *GetSecretResponse::deserialize_from_packet(get_response).map_err(anyhow_err)?;
        Ok(Some(get_response.secret.0))
    } else {
        let error = SecretkeeperError::deserialize_from_packet(get_response).map_err(anyhow_err)?;
        if *error == SecretkeeperError::EntryNotFound {
            return Ok(None);
        }
        Err(anyhow!("Secretkeeper get failed: {error:?}"))
    }
}

#[inline]
fn anyhow_err<E: core::fmt::Debug>(err: E) -> anyhow::Error {
    anyhow!("{:?}", err)
}

fn get_secretkeeper_service(
    host: &Strong<dyn IVirtualMachineService>,
) -> Result<Strong<dyn ISecretkeeper>> {
    Ok(host
        .getSecretkeeper()
        // TODO rename this error!
        .map_err(|e| {
            super::MicrodroidError::FailedToConnectToVirtualizationService(format!(
                "Failed to get Secretkeeper: {e:?}"
            ))
        })?)
}
