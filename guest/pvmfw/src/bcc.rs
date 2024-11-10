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

//! Code to inspect/manipulate the BCC (DICE Chain) we receive from our loader (the hypervisor).

// TODO(b/279910232): Unify this, somehow, with the similar but different code in hwtrust.

use alloc::vec;
use alloc::vec::Vec;
use ciborium::value::Value;
use core::fmt;
use core::mem::size_of;
use coset::{iana, Algorithm, CborSerializable, CoseKey};
use diced_open_dice::{BccHandover, Cdi, DiceArtifacts, DiceMode};
use log::trace;

type Result<T> = core::result::Result<T, BccError>;

pub enum BccError {
    CborDecodeError,
    CborEncodeError,
    CosetError(coset::CoseError),
    DiceError(diced_open_dice::DiceError),
    MalformedBcc(&'static str),
    MissingBcc,
}

impl From<coset::CoseError> for BccError {
    fn from(e: coset::CoseError) -> Self {
        Self::CosetError(e)
    }
}

impl fmt::Display for BccError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CborDecodeError => write!(f, "Error parsing BCC CBOR"),
            Self::CborEncodeError => write!(f, "Error encoding BCC CBOR"),
            Self::CosetError(e) => write!(f, "Encountered an error with coset: {e}"),
            Self::DiceError(e) => write!(f, "Dice error: {e:?}"),
            Self::MalformedBcc(s) => {
                write!(f, "BCC does not have the expected CBOR structure: {s}")
            }
            Self::MissingBcc => write!(f, "Missing BCC"),
        }
    }
}

/// Return a new CBOR encoded BccHandover that is based on the incoming CDIs but does not chain
/// from the received BCC.
pub fn truncate(bcc_handover: BccHandover) -> Result<Vec<u8>> {
    // Note: The strings here are deliberately different from those used in a normal DICE handover
    // because we want this to not be equivalent to any valid DICE derivation.
    let cdi_seal = taint_cdi(bcc_handover.cdi_seal(), "TaintCdiSeal")?;
    let cdi_attest = taint_cdi(bcc_handover.cdi_attest(), "TaintCdiAttest")?;

    // BccHandover = {
    //   1 : bstr .size 32,     ; CDI_Attest
    //   2 : bstr .size 32,     ; CDI_Seal
    //   ? 3 : Bcc,             ; Certificate chain
    // }
    let bcc_handover: Vec<(Value, Value)> =
        vec![(1.into(), cdi_attest.as_slice().into()), (2.into(), cdi_seal.as_slice().into())];
    cbor_util::serialize(&bcc_handover).map_err(|_| BccError::CborEncodeError)
}

fn taint_cdi(cdi: &Cdi, info: &str) -> Result<Cdi> {
    // An arbitrary value generated randomly.
    const SALT: [u8; 64] = [
        0xdc, 0x0d, 0xe7, 0x40, 0x47, 0x9d, 0x71, 0xb8, 0x69, 0xd0, 0x71, 0x85, 0x27, 0x47, 0xf5,
        0x65, 0x7f, 0x16, 0xfa, 0x59, 0x23, 0x19, 0x6a, 0x6b, 0x77, 0x41, 0x01, 0x45, 0x90, 0x3b,
        0xfa, 0x68, 0xad, 0xe5, 0x26, 0x31, 0x5b, 0x40, 0x85, 0x71, 0x97, 0x12, 0xbd, 0x0b, 0x38,
        0x5c, 0x98, 0xf3, 0x0e, 0xe1, 0x7c, 0x82, 0x23, 0xa4, 0x38, 0x38, 0x85, 0x84, 0x85, 0x0d,
        0x02, 0x90, 0x60, 0xd3,
    ];
    let mut result = [0u8; size_of::<Cdi>()];
    diced_open_dice::kdf(cdi.as_slice(), &SALT, info.as_bytes(), result.as_mut_slice())
        .map_err(BccError::DiceError)?;
    Ok(result)
}

/// Represents a (partially) decoded BCC DICE chain.
pub struct Bcc {
    is_debug_mode: bool,
    leaf_subject_pubkey: PublicKey,
}

impl Bcc {
    /// Returns whether any node in the received DICE chain is marked as debug (and hence is not
    /// secure).
    pub fn new(received_bcc: Option<&[u8]>) -> Result<Bcc> {
        let received_bcc = received_bcc.unwrap_or(&[]);
        if received_bcc.is_empty() {
            return Err(BccError::MissingBcc);
        }

        // We don't attempt to fully validate the BCC (e.g. we don't check the signatures) - we
        // have to trust our loader. But if it's invalid CBOR or otherwise clearly ill-formed,
        // something is very wrong, so we fail.
        let bcc_cbor =
            cbor_util::deserialize(received_bcc).map_err(|_| BccError::CborDecodeError)?;

        // Bcc = [
        //   PubKeyEd25519 / PubKeyECDSA256, // DK_pub
        //   + BccEntry,                     // Root -> leaf (KM_pub)
        // ]
        let bcc = match bcc_cbor {
            Value::Array(v) if v.len() >= 2 => v,
            _ => return Err(BccError::MalformedBcc("Invalid top level value")),
        };
        // Decode all the DICE payloads to make sure they are well-formed.
        let payloads = bcc
            .into_iter()
            .skip(1)
            .map(|v| BccEntry::new(v).payload())
            .collect::<Result<Vec<_>>>()?;

        let is_debug_mode = is_any_payload_debug_mode(&payloads)?;
        // Safe to unwrap because we checked the length above.
        let leaf_subject_pubkey = payloads.last().unwrap().subject_public_key()?;
        Ok(Self { is_debug_mode, leaf_subject_pubkey })
    }

    pub fn is_debug_mode(&self) -> bool {
        self.is_debug_mode
    }

    pub fn leaf_subject_pubkey(&self) -> &PublicKey {
        &self.leaf_subject_pubkey
    }
}

fn is_any_payload_debug_mode(payloads: &[BccPayload]) -> Result<bool> {
    // Check if any payload in the chain is marked as Debug mode, which means the device is not
    // secure. (Normal means it is a secure boot, for that stage at least; we ignore recovery
    // & not configured /invalid values, since it's not clear what they would mean in this
    // context.)
    for payload in payloads {
        if payload.is_debug_mode()? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[repr(transparent)]
struct BccEntry(Value);

#[repr(transparent)]
struct BccPayload(Value);

#[derive(Debug, Clone)]
pub struct PublicKey {
    /// The COSE key algorithm for the public key, representing the value of the `alg`
    /// field in the COSE key format of the public key. See RFC 8152, section 7 for details.
    pub cose_alg: iana::Algorithm,
}

impl BccEntry {
    pub fn new(entry: Value) -> Self {
        Self(entry)
    }

    pub fn payload(&self) -> Result<BccPayload> {
        // BccEntry = [                                  // COSE_Sign1 (untagged)
        //     protected : bstr .cbor {
        //         1 : AlgorithmEdDSA / AlgorithmES256,  // Algorithm
        //     },
        //     unprotected: {},
        //     payload: bstr .cbor BccPayload,
        //     signature: bstr // PureEd25519(SigningKey, bstr .cbor BccEntryInput) /
        //                     // ECDSA(SigningKey, bstr .cbor BccEntryInput)
        //     // See RFC 8032 for details of how to encode the signature value for Ed25519.
        // ]
        let payload =
            self.payload_bytes().ok_or(BccError::MalformedBcc("Invalid payload in BccEntry"))?;
        let payload = cbor_util::deserialize(payload).map_err(|_| BccError::CborDecodeError)?;
        trace!("Bcc payload: {payload:?}");
        Ok(BccPayload(payload))
    }

    fn payload_bytes(&self) -> Option<&Vec<u8>> {
        let entry = self.0.as_array()?;
        if entry.len() != 4 {
            return None;
        };
        entry[2].as_bytes()
    }
}

const KEY_MODE: i32 = -4670551;
const MODE_DEBUG: u8 = DiceMode::kDiceModeDebug as u8;
const SUBJECT_PUBLIC_KEY: i32 = -4670552;

impl BccPayload {
    pub fn is_debug_mode(&self) -> Result<bool> {
        // BccPayload = {                     // CWT
        // ...
        //     ? -4670551 : bstr,             // Mode
        // ...
        // }

        let Some(value) = self.value_from_key(KEY_MODE) else { return Ok(false) };

        // Mode is supposed to be encoded as a 1-byte bstr, but some implementations instead
        // encode it as an integer. Accept either. See b/273552826.
        // If Mode is omitted, it should be treated as if it was Unknown, according to the Open
        // Profile for DICE spec.
        let mode = if let Some(bytes) = value.as_bytes() {
            if bytes.len() != 1 {
                return Err(BccError::MalformedBcc("Invalid mode bstr"));
            }
            bytes[0].into()
        } else {
            value.as_integer().ok_or(BccError::MalformedBcc("Invalid type for mode"))?
        };
        Ok(mode == MODE_DEBUG.into())
    }

    fn subject_public_key(&self) -> Result<PublicKey> {
        // BccPayload = {                             ; CWT [RFC8392]
        // ...
        //   -4670552 : bstr .cbor PubKeyEd25519 /
        //              bstr .cbor PubKeyECDSA256 /
        //              bstr .cbor PubKeyECDSA384,    ; Subject Public Key
        // ...
        // }
        self.value_from_key(SUBJECT_PUBLIC_KEY)
            .ok_or(BccError::MalformedBcc("Subject public key missing"))?
            .as_bytes()
            .ok_or(BccError::MalformedBcc("Subject public key is not a byte string"))
            .and_then(|v| PublicKey::from_slice(v))
    }

    fn value_from_key(&self, key: i32) -> Option<&Value> {
        // BccPayload is just a map; we only use integral keys, but in general it's legitimate
        // for other things to be present, or for the key we care about not to be present.
        // Ciborium represents the map as a Vec, preserving order (and allowing duplicate keys,
        // which we ignore) but preventing fast lookup.
        let payload = self.0.as_map()?;
        for (k, v) in payload {
            if k.as_integer() == Some(key.into()) {
                return Some(v);
            }
        }
        None
    }
}

impl PublicKey {
    fn from_slice(slice: &[u8]) -> Result<Self> {
        let key = CoseKey::from_slice(slice)?;
        let Some(Algorithm::Assigned(cose_alg)) = key.alg else {
            return Err(BccError::MalformedBcc("Invalid algorithm in public key"));
        };
        Ok(Self { cose_alg })
    }
}
