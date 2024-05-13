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

//! Errors and relating structs thrown by the BoringSSL wrapper library.

#![cfg_attr(not(feature = "std"), no_std)]

mod code;

use core::{fmt, result};
use serde::{Deserialize, Serialize};

pub use crate::code::{CipherError, EcError, EcdsaError, GlobalError, ReasonCode};

/// libbssl_avf result type.
pub type Result<T> = result::Result<T, Error>;

/// Error type used by libbssl_avf.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Error {
    /// Failed to invoke a BoringSSL API.
    CallFailed(ApiName, ReasonCode),

    /// An unexpected internal error occurred.
    InternalError,

    /// Failed to decode the COSE_Key.
    CoseKeyDecodingFailed,

    /// An error occurred when interacting with the coset crate.
    CosetError,

    /// Unimplemented operation.
    Unimplemented,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::CallFailed(api_name, reason) => {
                write!(f, "Failed to invoke the BoringSSL API: {api_name:?}. Reason: {reason}")
            }
            Self::InternalError => write!(f, "An unexpected internal error occurred"),
            Self::CoseKeyDecodingFailed => write!(f, "Failed to decode the COSE_Key"),
            Self::CosetError => {
                write!(f, "An error occurred when interacting with the coset crate")
            }
            Self::Unimplemented => write!(f, "Unimplemented operation"),
        }
    }
}

impl From<coset::CoseError> for Error {
    fn from(e: coset::CoseError) -> Self {
        log::error!("Coset error: {e}");
        Self::CosetError
    }
}

/// BoringSSL API names.
#[allow(missing_docs)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiName {
    BN_new,
    BN_bin2bn,
    BN_bn2bin_padded,
    CBB_flush,
    CBB_len,
    EC_GROUP_new_by_curve_name,
    EC_KEY_check_key,
    EC_KEY_generate_key,
    EC_KEY_get0_group,
    EC_KEY_get0_public_key,
    EC_KEY_marshal_private_key,
    EC_KEY_parse_private_key,
    EC_KEY_new_by_curve_name,
    EC_KEY_set_public_key_affine_coordinates,
    EC_POINT_get_affine_coordinates,
    ECDSA_SIG_from_bytes,
    ECDSA_SIG_new,
    ECDSA_SIG_set0,
    ECDSA_sign,
    ECDSA_size,
    ECDSA_verify,
    ED25519_verify,
    EVP_AEAD_CTX_new,
    EVP_AEAD_CTX_open,
    EVP_AEAD_CTX_seal,
    EVP_Digest,
    EVP_MD_CTX_new,
    EVP_PKEY_new,
    EVP_PKEY_new_raw_public_key,
    EVP_PKEY_set1_EC_KEY,
    EVP_marshal_public_key,
    EVP_DigestVerify,
    EVP_DigestVerifyInit,
    HKDF,
    HMAC,
    i2d_ECDSA_SIG,
    RAND_bytes,
    SHA256,
}
