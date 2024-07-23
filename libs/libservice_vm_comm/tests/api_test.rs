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

use diced_open_dice::DiceArtifacts;
use service_vm_comm::{Csr, CsrPayload};

/// The following test data are generated with urandom
const DATA1: [u8; 32] = [
    0x8b, 0x09, 0xc0, 0x7e, 0x20, 0x3c, 0xa2, 0x11, 0x7e, 0x7f, 0x0b, 0xdd, 0x2b, 0x68, 0x98, 0xb0,
    0x2b, 0x34, 0xb5, 0x63, 0x39, 0x01, 0x90, 0x06, 0xaf, 0x5f, 0xdd, 0xb7, 0x81, 0xca, 0xc7, 0x46,
];
const DATA2: [u8; 16] = [
    0x6c, 0xb9, 0x39, 0x86, 0x9b, 0x2f, 0x12, 0xd8, 0x45, 0x92, 0x57, 0x44, 0x65, 0xce, 0x94, 0x63,
];

#[test]
fn csr_payload_cbor_serialization() {
    let csr_payload = CsrPayload { public_key: DATA1.to_vec(), challenge: DATA2.to_vec() };
    let expected_csr_payload = csr_payload.clone();
    let cbor_vec = csr_payload.into_cbor_vec().unwrap();
    let deserialized_csr_payload = CsrPayload::from_cbor_slice(&cbor_vec).unwrap();

    assert_eq!(expected_csr_payload, deserialized_csr_payload);
}

#[test]
fn csr_cbor_serialization() {
    let dice_artifacts = diced_sample_inputs::make_sample_bcc_and_cdis().unwrap();
    let dice_cert_chain = dice_artifacts.bcc().unwrap().to_vec();
    let csr = Csr { signed_csr_payload: DATA1.to_vec(), dice_cert_chain };
    let expected_csr = csr.clone();
    let cbor_vec = csr.into_cbor_vec().unwrap();
    let deserialized_csr = Csr::from_cbor_slice(&cbor_vec).unwrap();

    assert_eq!(expected_csr, deserialized_csr);
}
