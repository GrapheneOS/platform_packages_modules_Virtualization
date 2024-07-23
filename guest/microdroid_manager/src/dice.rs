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

use crate::instance::{ApexData, ApkData};
use crate::{is_debuggable, is_strict_boot, MicrodroidData};
use anyhow::{bail, Context, Result};
use ciborium::{cbor, Value};
use coset::CborSerializable;
use dice_driver::DiceDriver;
use diced_open_dice::{Hidden, OwnedDiceArtifacts, HIDDEN_SIZE};
use microdroid_metadata::PayloadMetadata;
use openssl::sha::{sha512, Sha512};
use std::iter::once;

/// Perform an open DICE derivation for the payload.
pub fn dice_derivation(
    dice: DiceDriver,
    instance_data: &MicrodroidData,
    payload_metadata: &PayloadMetadata,
) -> Result<OwnedDiceArtifacts> {
    let subcomponents = build_subcomponent_list(instance_data);
    let config_descriptor = format_payload_config_descriptor(payload_metadata, subcomponents)
        .context("Building config descriptor")?;

    // Calculate compound digests of code and authorities
    let mut code_hash_ctx = Sha512::new();
    let mut authority_hash_ctx = Sha512::new();
    code_hash_ctx.update(instance_data.apk_data.root_hash.as_ref());
    authority_hash_ctx.update(instance_data.apk_data.cert_hash.as_ref());
    for extra_apk in &instance_data.extra_apks_data {
        code_hash_ctx.update(extra_apk.root_hash.as_ref());
        authority_hash_ctx.update(extra_apk.cert_hash.as_ref());
    }
    for apex in &instance_data.apex_data {
        code_hash_ctx.update(apex.root_digest.as_ref());
        authority_hash_ctx.update(apex.public_key.as_ref());
    }
    let code_hash = code_hash_ctx.finish();
    let authority_hash = authority_hash_ctx.finish();

    // Check debuggability, conservatively assuming it is debuggable
    let debuggable = is_debuggable()?;

    // Send the details to diced
    let hidden = if cfg!(llpvm_changes) {
        hidden_input_from_instance_id()?
    } else {
        instance_data.salt.clone().try_into().unwrap()
    };
    dice.derive(code_hash, &config_descriptor, authority_hash, debuggable, hidden)
}

// Get the "Hidden input" for DICE derivation.
// This provides differentiation of secrets for different VM instances with same payload.
fn hidden_input_from_instance_id() -> Result<Hidden> {
    // For protected VM: this is all 0s, pvmfw ensures differentiation is added early in secrets.
    // For non-protected VM: this is derived from instance_id of the VM instance.
    let hidden_input = if !is_strict_boot() {
        if let Some(id) = super::get_instance_id()? {
            sha512(&id)
        } else {
            // TODO(b/325094712): Absence of instance_id occurs due to missing DT in some
            // x86_64 test devices (such as Cuttlefish). From security perspective, this is
            // acceptable for non-protected VM.
            log::warn!(
                "Instance Id missing, this may lead to 2 non protected VMs having same secrets"
            );
            [0u8; HIDDEN_SIZE]
        }
    } else {
        [0u8; HIDDEN_SIZE]
    };
    Ok(hidden_input)
}

struct Subcomponent {
    name: String,
    version: u64,
    code_hash: Vec<u8>,
    authority_hash: Vec<u8>,
}

impl Subcomponent {
    fn into_value(self) -> Result<Value> {
        Ok(cbor!({
           1 => self.name,
           2 => self.version,
           3 => Value::Bytes(self.code_hash),
           4 => Value::Bytes(self.authority_hash),
        })?)
    }

    fn for_apk(apk: &ApkData) -> Self {
        Self {
            name: format!("apk:{}", apk.package_name),
            version: apk.version_code,
            code_hash: apk.root_hash.clone(),
            authority_hash: apk.cert_hash.clone(),
        }
    }

    fn for_apex(apex: &ApexData) -> Self {
        // Note that this is only reachable if the dice_changes flag is on, in which case
        // the manifest data will always be present.
        Self {
            name: format!("apex:{}", apex.manifest_name.as_ref().unwrap()),
            version: apex.manifest_version.unwrap() as u64,
            code_hash: apex.root_digest.clone(),
            authority_hash: sha512(&apex.public_key).to_vec(),
        }
    }
}

fn build_subcomponent_list(instance_data: &MicrodroidData) -> Vec<Subcomponent> {
    if !cfg!(dice_changes) {
        return vec![];
    }

    let apks = once(&instance_data.apk_data)
        .chain(&instance_data.extra_apks_data)
        .map(Subcomponent::for_apk);
    let apexes = instance_data.apex_data.iter().map(Subcomponent::for_apex);
    apks.chain(apexes).collect()
}

// Returns a configuration descriptor of the given payload. See dice_for_avf_guest.cddl for the
// definition of the format.
fn format_payload_config_descriptor(
    payload: &PayloadMetadata,
    subcomponents: Vec<Subcomponent>,
) -> Result<Vec<u8>> {
    let mut map = Vec::new();
    map.push((cbor!(-70002)?, cbor!("Microdroid payload")?));
    map.push(match payload {
        PayloadMetadata::ConfigPath(payload_config_path) => {
            (cbor!(-71000)?, cbor!(payload_config_path)?)
        }
        PayloadMetadata::Config(payload_config) => {
            (cbor!(-71001)?, cbor!({1 => payload_config.payload_binary_name})?)
        }
        _ => bail!("Failed to match the payload against a config type: {:?}", payload),
    });

    if !subcomponents.is_empty() {
        let values =
            subcomponents.into_iter().map(Subcomponent::into_value).collect::<Result<Vec<_>>>()?;
        map.push((cbor!(-71002)?, cbor!(values)?));
    }

    Ok(Value::Map(map).to_vec()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use microdroid_metadata::PayloadConfig;

    const NO_SUBCOMPONENTS: Vec<Subcomponent> = Vec::new();

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
    fn payload_metadata_with_path_formats_correctly() -> Result<()> {
        let payload_metadata = PayloadMetadata::ConfigPath("/config_path".to_string());
        let config_descriptor =
            format_payload_config_descriptor(&payload_metadata, NO_SUBCOMPONENTS)?;
        static EXPECTED_CONFIG_DESCRIPTOR: &[u8] = &[
            0xa2, 0x3a, 0x00, 0x01, 0x11, 0x71, 0x72, 0x4d, 0x69, 0x63, 0x72, 0x6f, 0x64, 0x72,
            0x6f, 0x69, 0x64, 0x20, 0x70, 0x61, 0x79, 0x6c, 0x6f, 0x61, 0x64, 0x3a, 0x00, 0x01,
            0x15, 0x57, 0x6c, 0x2f, 0x63, 0x6f, 0x6e, 0x66, 0x69, 0x67, 0x5f, 0x70, 0x61, 0x74,
            0x68,
        ];
        assert_eq_bytes(EXPECTED_CONFIG_DESCRIPTOR, &config_descriptor);
        Ok(())
    }

    #[test]
    fn payload_metadata_with_config_formats_correctly() -> Result<()> {
        let payload_config = PayloadConfig {
            payload_binary_name: "payload_binary".to_string(),
            ..Default::default()
        };
        let payload_metadata = PayloadMetadata::Config(payload_config);
        let config_descriptor =
            format_payload_config_descriptor(&payload_metadata, NO_SUBCOMPONENTS)?;
        static EXPECTED_CONFIG_DESCRIPTOR: &[u8] = &[
            0xa2, 0x3a, 0x00, 0x01, 0x11, 0x71, 0x72, 0x4d, 0x69, 0x63, 0x72, 0x6f, 0x64, 0x72,
            0x6f, 0x69, 0x64, 0x20, 0x70, 0x61, 0x79, 0x6c, 0x6f, 0x61, 0x64, 0x3a, 0x00, 0x01,
            0x15, 0x58, 0xa1, 0x01, 0x6e, 0x70, 0x61, 0x79, 0x6c, 0x6f, 0x61, 0x64, 0x5f, 0x62,
            0x69, 0x6e, 0x61, 0x72, 0x79,
        ];
        assert_eq_bytes(EXPECTED_CONFIG_DESCRIPTOR, &config_descriptor);
        Ok(())
    }

    #[test]
    fn payload_metadata_with_subcomponents_formats_correctly() -> Result<()> {
        let payload_metadata = PayloadMetadata::ConfigPath("/config_path".to_string());
        let subcomponents = vec![
            Subcomponent {
                name: "apk1".to_string(),
                version: 1,
                code_hash: vec![42, 43],
                authority_hash: vec![17],
            },
            Subcomponent {
                name: "apk2".to_string(),
                version: 0x1000_0000_0001,
                code_hash: vec![43],
                authority_hash: vec![19, 20],
            },
        ];
        let config_descriptor = format_payload_config_descriptor(&payload_metadata, subcomponents)?;
        // Verified using cbor.me.
        static EXPECTED_CONFIG_DESCRIPTOR: &[u8] = &[
            0xa3, 0x3a, 0x00, 0x01, 0x11, 0x71, 0x72, 0x4d, 0x69, 0x63, 0x72, 0x6f, 0x64, 0x72,
            0x6f, 0x69, 0x64, 0x20, 0x70, 0x61, 0x79, 0x6c, 0x6f, 0x61, 0x64, 0x3a, 0x00, 0x01,
            0x15, 0x57, 0x6c, 0x2f, 0x63, 0x6f, 0x6e, 0x66, 0x69, 0x67, 0x5f, 0x70, 0x61, 0x74,
            0x68, 0x3a, 0x00, 0x01, 0x15, 0x59, 0x82, 0xa4, 0x01, 0x64, 0x61, 0x70, 0x6b, 0x31,
            0x02, 0x01, 0x03, 0x42, 0x2a, 0x2b, 0x04, 0x41, 0x11, 0xa4, 0x01, 0x64, 0x61, 0x70,
            0x6b, 0x32, 0x02, 0x1b, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x01, 0x03, 0x41,
            0x2b, 0x04, 0x42, 0x13, 0x14,
        ];
        assert_eq_bytes(EXPECTED_CONFIG_DESCRIPTOR, &config_descriptor);
        Ok(())
    }
}
