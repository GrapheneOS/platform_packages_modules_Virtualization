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

//! This module contains the structs related to the CSR (Certificate Signing Request)
//! sent from the client VM to the service VM for attestation.

use alloc::vec;
use alloc::vec::Vec;
use cbor_util::{cbor_value_type, value_to_bytes};
use ciborium::Value;
use coset::{self, CborSerializable, CoseError};

/// Represents a CSR sent from the client VM to the service VM for attestation.
///
/// See client_vm_csr.cddl for the definition of the CSR.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Csr {
    /// The DICE certificate chain of the client VM.
    pub dice_cert_chain: Vec<u8>,

    /// The signed CSR payload in COSE_Sign structure, which includes two signatures:
    /// - one by CDI_Leaf_Priv of the client VM's DICE chain,
    /// - another by the private key corresponding to the public key.
    pub signed_csr_payload: Vec<u8>,
}

impl Csr {
    /// Serializes this object to a CBOR-encoded vector.
    pub fn into_cbor_vec(self) -> coset::Result<Vec<u8>> {
        let value = Value::Array(vec![
            Value::Bytes(self.dice_cert_chain),
            Value::Bytes(self.signed_csr_payload),
        ]);
        value.to_vec()
    }

    /// Creates an object instance from the provided CBOR-encoded slice.
    pub fn from_cbor_slice(data: &[u8]) -> coset::Result<Self> {
        let value = Value::from_slice(data)?;
        let Value::Array(mut arr) = value else {
            return Err(CoseError::UnexpectedItem(cbor_value_type(&value), "array"));
        };
        if arr.len() != 2 {
            return Err(CoseError::UnexpectedItem("array", "array with 2 items"));
        }
        Ok(Self {
            signed_csr_payload: value_to_bytes(arr.remove(1), "signed_csr_payload")?,
            dice_cert_chain: value_to_bytes(arr.remove(0), "dice_cert_chain")?,
        })
    }
}

/// Represents the data to be signed and sent from the client VM to the service VM
/// for attestation.
///
/// It will be signed by both CDI_Leaf_Priv of the client VM's DICE chain and
/// the private key corresponding to the public key to be attested.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CsrPayload {
    /// COSE_Key encoded EC P-256 public key to be attested.
    pub public_key: Vec<u8>,

    /// A random array with a length between 0 and 64.
    /// It will be included in the certificate chain in the attestation result,
    /// serving as proof of the freshness of the result.
    pub challenge: Vec<u8>,
}

impl CsrPayload {
    /// Serializes this object to a CBOR-encoded vector.
    pub fn into_cbor_vec(self) -> coset::Result<Vec<u8>> {
        let value = Value::Array(vec![Value::Bytes(self.public_key), Value::Bytes(self.challenge)]);
        value.to_vec()
    }

    /// Creates an object instance from the provided CBOR-encoded slice.
    pub fn from_cbor_slice(data: &[u8]) -> coset::Result<Self> {
        let value = Value::from_slice(data)?;
        let Value::Array(mut arr) = value else {
            return Err(CoseError::UnexpectedItem(cbor_value_type(&value), "array"));
        };
        if arr.len() != 2 {
            return Err(CoseError::UnexpectedItem("array", "array with 2 items"));
        }
        Ok(Self {
            challenge: value_to_bytes(arr.remove(1), "challenge")?,
            public_key: value_to_bytes(arr.remove(0), "public_key")?,
        })
    }
}
