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

//! Wrappers of the AEAD functions in BoringSSL aead.h.

use crate::util::{check_int_result, to_call_failed_error};
use bssl_avf_error::{ApiName, Result};
use bssl_sys::{
    EVP_AEAD_CTX_free, EVP_AEAD_CTX_new, EVP_AEAD_CTX_open, EVP_AEAD_CTX_seal,
    EVP_AEAD_max_overhead, EVP_AEAD_nonce_length, EVP_aead_aes_256_gcm,
    EVP_aead_aes_256_gcm_randnonce, EVP_AEAD, EVP_AEAD_CTX, EVP_AEAD_DEFAULT_TAG_LENGTH,
};
use core::ptr::NonNull;

/// BoringSSL spec recommends to use 12-byte nonces.
///
/// https://commondatastorage.googleapis.com/chromium-boringssl-docs/aead.h.html#EVP_aead_aes_256_gcm
pub const AES_GCM_NONCE_LENGTH: usize = 12;

/// Magic value indicating that the default tag length for an AEAD should be used to
/// initialize `AeadContext`.
const AEAD_DEFAULT_TAG_LENGTH: usize = EVP_AEAD_DEFAULT_TAG_LENGTH as usize;

/// Represents an AEAD algorithm.
#[derive(Clone, Copy, Debug)]
pub struct Aead(&'static EVP_AEAD);

impl Aead {
    /// This is AES-256 in Galois Counter Mode.
    /// AES-GCM should only be used with 12-byte (96-bit) nonces as suggested in the
    /// BoringSSL spec:
    ///
    /// https://commondatastorage.googleapis.com/chromium-boringssl-docs/aead.h.html
    pub fn aes_256_gcm() -> Self {
        // SAFETY: This function does not access any Rust variables and simply returns
        // a pointer to the static variable in BoringSSL.
        let p = unsafe { EVP_aead_aes_256_gcm() };
        // SAFETY: The returned pointer should always be valid and points to a static
        // `EVP_AEAD`.
        Self(unsafe { &*p })
    }

    /// AES-256 in Galois Counter Mode with internal nonce generation.
    /// The 12-byte nonce is appended to the tag and is generated internally.
    pub fn aes_256_gcm_randnonce() -> Self {
        // SAFETY: This function does not access any Rust variables and simply returns
        // a pointer to the static variable in BoringSSL.
        let p = unsafe { EVP_aead_aes_256_gcm_randnonce() };
        // SAFETY: The returned pointer should always be valid and points to a static
        // `EVP_AEAD`.
        Self(unsafe { &*p })
    }

    /// Returns the maximum number of additional bytes added by the act of sealing data.
    pub fn max_overhead(&self) -> usize {
        // SAFETY: This function only reads from self.
        unsafe { EVP_AEAD_max_overhead(self.0) }
    }

    /// Returns the length, in bytes, of the per-message nonce.
    pub fn nonce_length(&self) -> usize {
        // SAFETY: This function only reads from self.
        unsafe { EVP_AEAD_nonce_length(self.0) }
    }
}

/// Represents an AEAD algorithm configuration.
pub struct AeadContext {
    ctx: NonNull<EVP_AEAD_CTX>,
    aead: Aead,
}

impl Drop for AeadContext {
    fn drop(&mut self) {
        // SAFETY: It is safe because the pointer has been created with `EVP_AEAD_CTX_new`
        // and isn't used after this.
        unsafe { EVP_AEAD_CTX_free(self.ctx.as_ptr()) }
    }
}

impl AeadContext {
    /// Creates a new `AeadContext` with the given `Aead` algorithm, `key` and `tag_len`.
    ///
    /// The default tag length will be used if `tag_len` is None.
    pub fn new(aead: Aead, key: &[u8], tag_len: Option<usize>) -> Result<Self> {
        let tag_len = tag_len.unwrap_or(AEAD_DEFAULT_TAG_LENGTH);
        // SAFETY: This function only reads the given data and the returned pointer is
        // checked below.
        let ctx = unsafe { EVP_AEAD_CTX_new(aead.0, key.as_ptr(), key.len(), tag_len) };
        let ctx =
            NonNull::new(ctx).ok_or_else(|| to_call_failed_error(ApiName::EVP_AEAD_CTX_new))?;
        Ok(Self { ctx, aead })
    }

    /// Encrypts and authenticates `data` and writes the result to `out`.
    /// The `out` length should be at least the `data` length plus the `max_overhead` of the
    /// `aead` and the length of `nonce` should match the `nonce_length` of the `aead`.
    ///  Otherwise, an error will be returned.
    ///
    /// The output is returned as a subslice of `out`.
    pub fn seal<'b>(
        &self,
        data: &[u8],
        nonce: &[u8],
        ad: &[u8],
        out: &'b mut [u8],
    ) -> Result<&'b [u8]> {
        let mut out_len = 0;
        // SAFETY: Only reads from/writes to the provided slices.
        let ret = unsafe {
            EVP_AEAD_CTX_seal(
                self.ctx.as_ptr(),
                out.as_mut_ptr(),
                &mut out_len,
                out.len(),
                nonce.as_ptr(),
                nonce.len(),
                data.as_ptr(),
                data.len(),
                ad.as_ptr(),
                ad.len(),
            )
        };
        check_int_result(ret, ApiName::EVP_AEAD_CTX_seal)?;
        out.get(0..out_len).ok_or_else(|| to_call_failed_error(ApiName::EVP_AEAD_CTX_seal))
    }

    /// Authenticates `data` and decrypts it to `out`.
    /// The `out` length should be at least the `data` length, and the length of `nonce` should
    /// match the `nonce_length` of the `aead`.
    /// Otherwise, an error will be returned.
    ///
    /// The output is returned as a subslice of `out`.
    pub fn open<'b>(
        &self,
        data: &[u8],
        nonce: &[u8],
        ad: &[u8],
        out: &'b mut [u8],
    ) -> Result<&'b [u8]> {
        let mut out_len = 0;
        // SAFETY: Only reads from/writes to the provided slices.
        // `data` and `out` are checked to be non-alias internally.
        let ret = unsafe {
            EVP_AEAD_CTX_open(
                self.ctx.as_ptr(),
                out.as_mut_ptr(),
                &mut out_len,
                out.len(),
                nonce.as_ptr(),
                nonce.len(),
                data.as_ptr(),
                data.len(),
                ad.as_ptr(),
                ad.len(),
            )
        };
        check_int_result(ret, ApiName::EVP_AEAD_CTX_open)?;
        out.get(0..out_len).ok_or_else(|| to_call_failed_error(ApiName::EVP_AEAD_CTX_open))
    }

    /// Returns the `Aead` represented by this `AeadContext`.
    pub fn aead(&self) -> Aead {
        self.aead
    }
}
