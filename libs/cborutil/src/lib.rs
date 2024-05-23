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

//! Utility functions for CBOR serialization/deserialization.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use ciborium::value::{Integer, Value};
use coset::{
    iana::{self, EnumI64},
    CborSerializable, CoseError, CoseKey, Label, Result,
};
use log::error;
use serde::{de::DeserializeOwned, Serialize};

/// Serializes the given data to a CBOR-encoded byte vector.
pub fn serialize<T: ?Sized + Serialize>(v: &T) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    ciborium::into_writer(v, &mut data)?;
    Ok(data)
}

/// Deserializes the given type from a CBOR-encoded byte slice, failing if any extra
/// data remains after the type has been read.
pub fn deserialize<T: DeserializeOwned>(mut data: &[u8]) -> Result<T> {
    let res = ciborium::from_reader(&mut data)?;
    if data.is_empty() {
        Ok(res)
    } else {
        Err(CoseError::ExtraneousData)
    }
}

/// Parses the given CBOR-encoded byte slice as a value array.
pub fn parse_value_array(data: &[u8], context: &'static str) -> Result<Vec<Value>> {
    value_to_array(Value::from_slice(data)?, context)
}

/// Converts the provided value `v` to a value array.
pub fn value_to_array(v: Value, context: &'static str) -> Result<Vec<Value>> {
    v.into_array().map_err(|e| to_unexpected_item_error(&e, "array", context))
}

/// Converts the provided value `v` to a text string.
pub fn value_to_text(v: Value, context: &'static str) -> Result<String> {
    v.into_text().map_err(|e| to_unexpected_item_error(&e, "tstr", context))
}

/// Converts the provided value `v` to a map.
pub fn value_to_map(v: Value, context: &'static str) -> Result<Vec<(Value, Value)>> {
    v.into_map().map_err(|e| to_unexpected_item_error(&e, "map", context))
}

/// Converts the provided value `v` to a number.
pub fn value_to_num<T: TryFrom<Integer>>(v: Value, context: &'static str) -> Result<T> {
    let num = v.into_integer().map_err(|e| to_unexpected_item_error(&e, "int", context))?;
    num.try_into().map_err(|_| {
        error!("The provided value '{num:?}' is not a valid number: {context}");
        CoseError::OutOfRangeIntegerValue
    })
}

/// Converts the provided value `v` to a byte array of length `N`.
pub fn value_to_byte_array<const N: usize>(v: Value, context: &'static str) -> Result<[u8; N]> {
    let arr = value_to_bytes(v, context)?;
    arr.try_into().map_err(|e| {
        error!("The provided value '{context}' is not an array of length {N}: {e:?}");
        CoseError::UnexpectedItem("bstr", "array of length {N}")
    })
}

/// Converts the provided value `v` to bytes array.
pub fn value_to_bytes(v: Value, context: &'static str) -> Result<Vec<u8>> {
    v.into_bytes().map_err(|e| to_unexpected_item_error(&e, "bstr", context))
}

/// Builds a `CoseError::UnexpectedItem` error when the provided value `v` is not of the expected
/// type `expected_type` and logs the error message with the provided `context`.
pub fn to_unexpected_item_error(
    v: &Value,
    expected_type: &'static str,
    context: &'static str,
) -> CoseError {
    let v_type = cbor_value_type(v);
    assert!(v_type != expected_type);
    error!("The provided value type '{v_type}' is not of type '{expected_type}': {context}");
    CoseError::UnexpectedItem(v_type, expected_type)
}

/// Reads the type of the provided value `v`.
pub fn cbor_value_type(v: &Value) -> &'static str {
    match v {
        Value::Integer(_) => "int",
        Value::Bytes(_) => "bstr",
        Value::Float(_) => "float",
        Value::Text(_) => "tstr",
        Value::Bool(_) => "bool",
        Value::Null => "nul",
        Value::Tag(_, _) => "tag",
        Value::Array(_) => "array",
        Value::Map(_) => "map",
        _ => "other",
    }
}

/// Returns the value of the given label in the given COSE key as bytes.
pub fn get_label_value_as_bytes(key: &CoseKey, label: Label) -> Result<&[u8]> {
    let v = get_label_value(key, label)?;
    Ok(v.as_bytes().ok_or_else(|| {
        to_unexpected_item_error(v, "bstr", "Get label value in CoseKey as bytes")
    })?)
}

/// Returns the value of the given label in the given COSE key.
pub fn get_label_value(key: &CoseKey, label: Label) -> Result<&Value> {
    Ok(&key
        .params
        .iter()
        .find(|(k, _)| k == &label)
        .ok_or(CoseError::UnexpectedItem("", "Label not found in CoseKey"))?
        .1)
}

/// Converts the provided COSE key algorithm integer to an `iana::Algorithm` used
/// by DICE chains.
pub fn dice_cose_key_alg(cose_key_alg: i32) -> Result<iana::Algorithm> {
    let key_alg = iana::Algorithm::from_i64(cose_key_alg as i64).ok_or_else(|| {
        error!("Unsupported COSE key algorithm for DICE: {cose_key_alg}");
        CoseError::UnexpectedItem("COSE key algorithm", "")
    })?;
    match key_alg {
        iana::Algorithm::EdDSA | iana::Algorithm::ES256 | iana::Algorithm::ES384 => Ok(key_alg),
        _ => {
            error!("Unsupported COSE key algorithm for DICE: {key_alg:?}");
            Err(CoseError::UnexpectedItem("-8, -7 or -35", ""))
        }
    }
}
