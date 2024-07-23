/*
 * Copyright (C) 2023 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Provides functions to build fake DICE artifacts for client VM in tests.

use crate::service_vm;
use alloc::vec;
use alloc::vec::Vec;
use ciborium::{cbor, value::Value};
use core::result;
use coset::CborSerializable;
use cstr::cstr;
use diced_open_dice::{
    hash, retry_bcc_format_config_descriptor, retry_bcc_main_flow, Config, DiceArtifacts,
    DiceConfigValues, DiceError, DiceMode, InputValues, OwnedDiceArtifacts, Result, HASH_SIZE,
    HIDDEN_SIZE,
};
use log::error;
use microdroid_kernel_hashes::OS_HASHES;

type CborResult<T> = result::Result<T, ciborium::value::Error>;

/// All the following data are generated with urandom.
const CODE_HASH_PAYLOAD: [u8; HASH_SIZE] = [
    0x08, 0x78, 0xc2, 0x5b, 0xe7, 0xea, 0x3d, 0x62, 0x70, 0x22, 0xd9, 0x1c, 0x4f, 0x3c, 0x2e, 0x2f,
    0x0f, 0x97, 0xa4, 0x6f, 0x6d, 0xd5, 0xe6, 0x4a, 0x6d, 0xbe, 0x34, 0x2e, 0x56, 0x04, 0xaf, 0xef,
    0x74, 0x3f, 0xec, 0xb8, 0x44, 0x11, 0xf4, 0x2f, 0x05, 0xb2, 0x06, 0xa3, 0x0e, 0x75, 0xb7, 0x40,
    0x9a, 0x4c, 0x58, 0xab, 0x96, 0xe7, 0x07, 0x97, 0x07, 0x86, 0x5c, 0xa1, 0x42, 0x12, 0xf0, 0x34,
];
const AUTHORITY_HASH_PAYLOAD: [u8; HASH_SIZE] = [
    0xc7, 0x97, 0x5b, 0xa9, 0x9e, 0xbf, 0x0b, 0xeb, 0xe7, 0x7f, 0x69, 0x8f, 0x8e, 0xcf, 0x04, 0x7d,
    0x2c, 0x0f, 0x4d, 0xbe, 0xcb, 0xf5, 0xf1, 0x4c, 0x1d, 0x1c, 0xb7, 0x44, 0xdf, 0xf8, 0x40, 0x90,
    0x09, 0x65, 0xab, 0x01, 0x34, 0x3e, 0xc2, 0xc4, 0xf7, 0xa2, 0x3a, 0x5c, 0x4e, 0x76, 0x4f, 0x42,
    0xa8, 0x6c, 0xc9, 0xf1, 0x7b, 0x12, 0x80, 0xa4, 0xef, 0xa2, 0x4d, 0x72, 0xa1, 0x21, 0xe2, 0x47,
];
const APK1_CODE_HASH: &[u8] = &[
    0x41, 0x92, 0x0d, 0xd0, 0xf5, 0x60, 0xe3, 0x69, 0x26, 0x7f, 0xb8, 0xbc, 0x12, 0x3a, 0xd1, 0x95,
    0x1d, 0xb8, 0x9a, 0x9c, 0x3a, 0x3f, 0x01, 0xbf, 0xa8, 0xd9, 0x6d, 0xe9, 0x90, 0x30, 0x1d, 0x0b,
];
const APK1_AUTHORITY_HASH: &[u8] = &[
    0xe3, 0xd9, 0x1c, 0xf5, 0x6f, 0xee, 0x73, 0x40, 0x3d, 0x95, 0x59, 0x67, 0xea, 0x5d, 0x01, 0xfd,
    0x25, 0x9d, 0x5c, 0x88, 0x94, 0x3a, 0xc6, 0xd7, 0xa9, 0xdc, 0x4c, 0x60, 0x81, 0xbe, 0x2b, 0x74,
];
const APEX1_CODE_HASH: &[u8] = &[
    0x52, 0x93, 0x2b, 0xb0, 0x8d, 0xec, 0xdf, 0x54, 0x1f, 0x5c, 0x10, 0x9d, 0x17, 0xce, 0x7f, 0xac,
    0xb0, 0x2b, 0xe2, 0x99, 0x05, 0x7d, 0xa3, 0x9b, 0xa6, 0x3e, 0xf9, 0x99, 0xa2, 0xea, 0xd4, 0xd9,
];
const APEX1_AUTHORITY_HASH: &[u8] = &[
    0xd1, 0xfc, 0x3d, 0x5f, 0xa0, 0x5f, 0x02, 0xd0, 0x83, 0x9b, 0x0e, 0x32, 0xc2, 0x27, 0x09, 0x12,
    0xcc, 0xfc, 0x42, 0xf6, 0x0d, 0xf4, 0x7d, 0xc8, 0x80, 0x1a, 0x64, 0x25, 0xa7, 0xfa, 0x4a, 0x37,
];

#[allow(missing_docs)]
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SubComponent {
    pub name: String,
    pub version: u64,
    pub code_hash: Vec<u8>,
    pub authority_hash: Vec<u8>,
}

impl SubComponent {
    fn to_value(&self) -> CborResult<Value> {
        Ok(cbor!({
           1 => self.name,
           2 => self.version,
           3 => Value::Bytes(self.code_hash.clone()),
           4 => Value::Bytes(self.authority_hash.clone()),
        })?)
    }
}

/// Generates fake DICE artifacts for client VM with a DICE chain up to the certificate
/// describing the Microdroid payload.
///
/// The fake DICE chain has the following nodes:
/// Root public key -> pvmfw certificate -> Microdroid kernel certificate
/// -> Microdroid payload certificate
pub fn fake_client_vm_dice_artifacts() -> Result<OwnedDiceArtifacts> {
    // Client VM DICE chain has the same prefix as the service VM DICE chain up to
    // the pvmfw entry.
    let (cdi_values, dice_chain) = service_vm::fake_dice_artifacts_up_to_pvmfw()?;

    // Adds an entry describing the Microdroid kernel.
    let config_values = DiceConfigValues {
        component_name: Some(cstr!("vm_entry")),
        component_version: Some(12),
        resettable: true,
        ..Default::default()
    };
    let config_descriptor = retry_bcc_format_config_descriptor(&config_values)?;
    // The Microdroid kernel is signed with the same key as the one used for the service VM,
    // so the authority hash is the same.
    let authority_hash = service_vm::AUTHORITY_HASH_SERVICE_VM;
    let input_values = InputValues::new(
        kernel_code_hash()?,
        Config::Descriptor(config_descriptor.as_slice()),
        authority_hash,
        DiceMode::kDiceModeDebug,
        [0; HIDDEN_SIZE], // No hidden.
    );
    let dice_artifacts = retry_bcc_main_flow(
        &cdi_values.cdi_attest,
        &cdi_values.cdi_seal,
        &dice_chain,
        &input_values,
    )
    .map_err(|e| {
        error!("Failed to run the Microdroid kernel BCC main flow: {e}");
        e
    })?;

    // Adds an entry describing the Microdroid payload.
    let config_descriptor = fake_microdroid_payload_config_descriptor().map_err(|e| {
        error!("Failed to generate config descriptor for Microdroid: {e}");
        DiceError::InvalidInput
    })?;
    let input_values = InputValues::new(
        CODE_HASH_PAYLOAD,
        Config::Descriptor(config_descriptor.as_slice()),
        AUTHORITY_HASH_PAYLOAD,
        DiceMode::kDiceModeDebug,
        [0u8; HIDDEN_SIZE], // hidden
    );
    retry_bcc_main_flow(
        dice_artifacts.cdi_attest(),
        dice_artifacts.cdi_seal(),
        dice_artifacts.bcc().unwrap(),
        &input_values,
    )
    .map_err(|e| {
        error!("Failed to run the Microdroid payload BCC main flow: {e}");
        e
    })
}

fn fake_microdroid_payload_config_descriptor() -> CborResult<Vec<u8>> {
    let mut map = Vec::new();
    map.push((cbor!(-70002)?, cbor!("Microdroid payload")?));
    map.push((cbor!(-71000)?, cbor!("/config_path")?));
    let components =
        fake_sub_components().iter().map(|c| c.to_value()).collect::<CborResult<_>>()?;
    map.push((cbor!(-71002)?, Value::Array(components)));
    Ok(Value::Map(map).to_vec().unwrap())
}

/// Generates a list of fake subcomponents as the Microdroid payload.
pub fn fake_sub_components() -> Vec<SubComponent> {
    vec![
        SubComponent {
            name: "apk:com.android.apk.apk1".to_string(),
            version: 1,
            code_hash: APK1_CODE_HASH.to_vec(),
            authority_hash: APK1_AUTHORITY_HASH.to_vec(),
        },
        SubComponent {
            name: "apex:com.android.apex.apex1".to_string(),
            version: 1,
            code_hash: APEX1_CODE_HASH.to_vec(),
            authority_hash: APEX1_AUTHORITY_HASH.to_vec(),
        },
    ]
}

fn kernel_code_hash() -> Result<[u8; HASH_SIZE]> {
    let os_hashes = &OS_HASHES[0];
    let code_hash = [os_hashes.kernel, os_hashes.initrd_debug].concat();
    hash(&code_hash)
}
