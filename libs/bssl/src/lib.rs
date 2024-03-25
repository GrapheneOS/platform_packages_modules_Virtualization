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

//! Safe wrappers around the BoringSSL API.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(clippy::or_fun_call)]

extern crate alloc;

mod aead;
mod cbb;
mod cbs;
mod curve25519;
mod digest;
mod ec_key;
mod err;
mod evp;
mod hkdf;
mod hmac;
mod rand;
mod sha;
mod util;

pub use bssl_avf_error::{ApiName, CipherError, EcError, EcdsaError, Error, ReasonCode, Result};

pub use aead::{Aead, AeadContext, AES_GCM_NONCE_LENGTH};
pub use cbb::CbbFixed;
pub use cbs::Cbs;
pub use curve25519::ed25519_verify;
pub use digest::Digester;
pub use ec_key::{EcKey, ZVec};
pub use evp::{PKey, PKeyType};
pub use hkdf::hkdf;
pub use hmac::hmac_sha256;
pub use rand::rand_bytes;
pub use sha::sha256;
