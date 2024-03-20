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

//! Wrappers of the EVP functions in BoringSSL evp.h.

use crate::cbb::CbbFixed;
use crate::digest::{Digester, DigesterContext};
use crate::ec_key::EcKey;
use crate::util::{check_int_result, to_call_failed_error};
use alloc::vec::Vec;
use bssl_avf_error::{ApiName, Error, Result};
use bssl_sys::{
    CBB_flush, CBB_len, EVP_DigestVerify, EVP_DigestVerifyInit, EVP_PKEY_free, EVP_PKEY_new,
    EVP_PKEY_new_raw_public_key, EVP_PKEY_set1_EC_KEY, EVP_marshal_public_key, EVP_PKEY,
    EVP_PKEY_ED25519, EVP_PKEY_X25519,
};
use cbor_util::{get_label_value, get_label_value_as_bytes};
use ciborium::Value;
use core::ptr::{self, NonNull};
use coset::{
    iana::{self, EnumI64},
    CoseKey, KeyType, Label,
};
use log::error;

/// Wrapper of an `EVP_PKEY` object, representing a public or private key.
pub struct PKey {
    pkey: NonNull<EVP_PKEY>,
    /// If this struct owns the inner EC key, the inner EC key should remain valid as
    /// long as the pointer to `EVP_PKEY` is valid.
    _inner_ec_key: Option<EcKey>,
}

impl Drop for PKey {
    fn drop(&mut self) {
        // SAFETY: It is safe because `EVP_PKEY` has been allocated by BoringSSL and isn't
        // used after this.
        unsafe { EVP_PKEY_free(self.pkey.as_ptr()) }
    }
}

/// Creates a new empty `EVP_PKEY`.
fn new_pkey() -> Result<NonNull<EVP_PKEY>> {
    // SAFETY: The returned pointer is checked below.
    let key = unsafe { EVP_PKEY_new() };
    NonNull::new(key).ok_or_else(|| to_call_failed_error(ApiName::EVP_PKEY_new))
}

impl TryFrom<EcKey> for PKey {
    type Error = bssl_avf_error::Error;

    fn try_from(key: EcKey) -> Result<Self> {
        let pkey = new_pkey()?;
        // SAFETY: The function only sets the inner EC key of the initialized and
        // non-null `EVP_PKEY` to point to the given `EC_KEY`. It only reads from
        // and writes to the initialized `EVP_PKEY`.
        // Since this struct owns the inner key, the inner key remains valid as
        // long as `EVP_PKEY` is valid.
        let ret = unsafe { EVP_PKEY_set1_EC_KEY(pkey.as_ptr(), key.0.as_ptr()) };
        check_int_result(ret, ApiName::EVP_PKEY_set1_EC_KEY)?;
        Ok(Self { pkey, _inner_ec_key: Some(key) })
    }
}

impl PKey {
    /// Returns a DER-encoded SubjectPublicKeyInfo structure as specified
    /// in RFC 5280 s4.1.2.7:
    ///
    /// https://www.rfc-editor.org/rfc/rfc5280.html#section-4.1.2.7
    pub fn subject_public_key_info(&self) -> Result<Vec<u8>> {
        const CAPACITY: usize = 256;
        let mut buf = [0u8; CAPACITY];
        let mut cbb = CbbFixed::new(buf.as_mut());
        // SAFETY: The function only write bytes to the buffer managed by the valid `CBB`.
        // The inner key in `EVP_PKEY` was set to a valid key when the object was created.
        // As this struct owns the inner key, the inner key is guaranteed to be valid
        // throughout the execution of the function.
        let ret = unsafe { EVP_marshal_public_key(cbb.as_mut(), self.pkey.as_ptr()) };
        check_int_result(ret, ApiName::EVP_marshal_public_key)?;
        // SAFETY: This is safe because the CBB pointer is a valid pointer initialized with
        // `CBB_init_fixed()`.
        check_int_result(unsafe { CBB_flush(cbb.as_mut()) }, ApiName::CBB_flush)?;
        // SAFETY: This is safe because the CBB pointer is initialized with `CBB_init_fixed()`,
        // and it has been flushed, thus it has no active children.
        let len = unsafe { CBB_len(cbb.as_ref()) };
        Ok(buf.get(0..len).ok_or_else(|| to_call_failed_error(ApiName::CBB_len))?.to_vec())
    }

    /// This function takes a raw public key data slice and creates a `PKey` instance wrapping
    /// a freshly allocated `EVP_PKEY` object from it.
    ///
    /// The lifetime of the returned instance is not tied to the lifetime of the raw public
    /// key slice because the raw data is copied into the `EVP_PKEY` object.
    ///
    /// Currently the only supported raw formats are X25519 and Ed25519, where the formats
    /// are specified in RFC 7748 and RFC 8032 respectively.
    pub fn new_raw_public_key(raw_public_key: &[u8], type_: PKeyType) -> Result<Self> {
        let engine = ptr::null_mut(); // Engine is not used.
        let pkey =
            // SAFETY: The function only reads from the given raw public key within its bounds.
            // The returned pointer is checked below.
            unsafe {
                EVP_PKEY_new_raw_public_key(
                    type_.0,
                    engine,
                    raw_public_key.as_ptr(),
                    raw_public_key.len(),
                )
            };
        let pkey = NonNull::new(pkey)
            .ok_or_else(|| to_call_failed_error(ApiName::EVP_PKEY_new_raw_public_key))?;
        Ok(Self { pkey, _inner_ec_key: None })
    }

    /// Creates a `PKey` from the given `cose_key`.
    ///
    /// The lifetime of the returned instance is not tied to the lifetime of the `cose_key` as the
    /// data of `cose_key` is copied into the `EVP_PKEY` or `EC_KEY` object.
    pub fn from_cose_public_key(cose_key: &CoseKey) -> Result<Self> {
        match &cose_key.kty {
            KeyType::Assigned(iana::KeyType::EC2) => {
                EcKey::from_cose_public_key(cose_key)?.try_into()
            }
            KeyType::Assigned(iana::KeyType::OKP) => {
                let curve_type =
                    get_label_value(cose_key, Label::Int(iana::OkpKeyParameter::Crv.to_i64()))?;
                let curve_type = match curve_type {
                    crv if crv == &Value::from(iana::EllipticCurve::Ed25519.to_i64()) => {
                        PKeyType::ED25519
                    }
                    crv if crv == &Value::from(iana::EllipticCurve::X25519.to_i64()) => {
                        PKeyType::X25519
                    }
                    crv => {
                        error!("Unsupported curve type in OKP COSE key: {:?}", crv);
                        return Err(Error::Unimplemented);
                    }
                };
                let x = get_label_value_as_bytes(
                    cose_key,
                    Label::Int(iana::OkpKeyParameter::X.to_i64()),
                )?;
                Self::new_raw_public_key(x, curve_type)
            }
            kty => {
                error!("Unsupported key type in COSE key: {:?}", kty);
                Err(Error::Unimplemented)
            }
        }
    }

    /// Verifies the given `signature` of the `message` using the current public key.
    ///
    /// The `message` will be hashed using the given `digester` before verification.
    ///
    /// For algorithms like Ed25519 that do not use pre-hashed inputs, the `digester` should
    /// be `None`.
    pub fn verify(
        &self,
        signature: &[u8],
        message: &[u8],
        digester: Option<Digester>,
    ) -> Result<()> {
        let mut digester_context = DigesterContext::new()?;
        // The `EVP_PKEY_CTX` is set to null as this function does not collect the context
        // during the verification.
        let pkey_context = ptr::null_mut();
        let engine = ptr::null_mut(); // Use the default engine.
        let ret =
            // SAFETY: All the non-null parameters passed to this function have been properly
            // initialized as required in the BoringSSL spec.
            unsafe {
                EVP_DigestVerifyInit(
                    digester_context.as_mut_ptr(),
                    pkey_context,
                    digester.map_or(ptr::null(), |d| d.0),
                    engine,
                    self.pkey.as_ptr(),
                )
            };
        check_int_result(ret, ApiName::EVP_DigestVerifyInit)?;

        // SAFETY: The function only reads from the given slices within their bounds.
        // The `EVP_MD_CTX` is successfully initialized before this call.
        let ret = unsafe {
            EVP_DigestVerify(
                digester_context.as_mut_ptr(),
                signature.as_ptr(),
                signature.len(),
                message.as_ptr(),
                message.len(),
            )
        };
        check_int_result(ret, ApiName::EVP_DigestVerify)
    }
}

/// Type of the keys supported by `PKey`.
///
/// It is a wrapper of the `EVP_PKEY_*` macros defined BoringSSL evp.h, which are the
/// NID values of the corresponding keys.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PKeyType(i32);

impl PKeyType {
    /// EVP_PKEY_X25519 / NID_X25519
    pub const X25519: PKeyType = PKeyType(EVP_PKEY_X25519);
    /// EVP_PKEY_ED25519 / NID_ED25519
    pub const ED25519: PKeyType = PKeyType(EVP_PKEY_ED25519);
}
