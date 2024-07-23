/*
 * Copyright 2024 The Android Open Source Project
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

use std::error::Error;
use std::ffi::{c_void, CStr};
use std::fmt::{self, Display};
use std::iter::FusedIterator;
use std::ptr::{self, NonNull};

use vm_payload_bindgen::{
    AVmAttestationResult, AVmAttestationResult_free, AVmAttestationResult_getCertificateAt,
    AVmAttestationResult_getCertificateCount, AVmAttestationResult_getPrivateKey,
    AVmAttestationResult_sign, AVmAttestationStatus, AVmAttestationStatus_toString,
    AVmPayload_requestAttestation, AVmPayload_requestAttestationForTesting,
};

/// Holds the result of a successful Virtual Machine attestation request.
/// See [`request_attestation`].
#[derive(Debug)]
pub struct AttestationResult {
    result: NonNull<AVmAttestationResult>,
}

/// Error type that can be returned from an unsuccessful Virtual Machine attestation request.
/// See [`request_attestation`].
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum AttestationError {
    /// The challenge size was not between 0 and 64 bytes (inclusive).
    InvalidChallenge,
    /// The attempt to attest the VM failed. A subsequent request may succeed.
    AttestationFailed,
    /// VM attestation is not supported in the current environment.
    AttestationUnsupported,
}

impl Error for AttestationError {}

impl Display for AttestationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let status = match self {
            Self::InvalidChallenge => AVmAttestationStatus::ATTESTATION_ERROR_INVALID_CHALLENGE,
            Self::AttestationFailed => AVmAttestationStatus::ATTESTATION_ERROR_ATTESTATION_FAILED,
            Self::AttestationUnsupported => AVmAttestationStatus::ATTESTATION_ERROR_UNSUPPORTED,
        };
        // SAFETY: AVmAttestationStatus_toString always returns a non-null pointer to a
        // nul-terminated C string with static lifetime (which is valid UTF-8).
        let c_str = unsafe { CStr::from_ptr(AVmAttestationStatus_toString(status)) };
        let str = c_str.to_str().expect("Invalid UTF-8 for AVmAttestationStatus");
        f.write_str(str)
    }
}

impl Drop for AttestationResult {
    fn drop(&mut self) {
        let ptr = self.result.as_ptr();

        // SAFETY: The `result` field is private, and only populated with a successful call to
        // `AVmPayload_requestAttestation`, and not freed elsewhere.
        unsafe { AVmAttestationResult_free(ptr) };
    }
}

// SAFETY: The API functions that accept the `AVmAttestationResult` pointer are all safe to call
// from any thread, including `AVmAttestationResult_free` which is called only on drop.
unsafe impl Send for AttestationResult {}

// SAFETY: There is no interior mutation here; any future functions that might mutate the data would
// require a non-const pointer and hence need `&mut self` here. The only existing such function is
// `AVmAttestationResult_free` where we take a mutable reference guaranteeing no other references
// exist. The raw API functions are safe to call from any thread.
unsafe impl Sync for AttestationResult {}

/// Requests the remote attestation of this VM.
///
/// On success the supplied [`challenge`] will be included in the certificate chain accessible from
/// the [`AttestationResult`]; this can be used as proof of the freshness of the attestation.
///
/// The challenge should be no more than 64 bytes long or the request will fail.
pub fn request_attestation(challenge: &[u8]) -> Result<AttestationResult, AttestationError> {
    let mut result: *mut AVmAttestationResult = ptr::null_mut();
    // SAFETY: We only read the challenge within its bounds and the function does not retain any
    // reference to it.
    let status = unsafe {
        AVmPayload_requestAttestation(
            challenge.as_ptr() as *const c_void,
            challenge.len(),
            &mut result,
        )
    };
    AttestationResult::new(status, result)
}

/// A variant of [`request_attestation`] used for testing purposes. This should not be used by
/// normal VMs, and is not available to app owned VMs.
pub fn request_attestation_for_testing(
    challenge: &[u8],
) -> Result<AttestationResult, AttestationError> {
    let mut result: *mut AVmAttestationResult = ptr::null_mut();
    // SAFETY: We only read the challenge within its bounds and the function does not retain any
    // reference to it.
    let status = unsafe {
        AVmPayload_requestAttestationForTesting(
            challenge.as_ptr() as *const c_void,
            challenge.len(),
            &mut result,
        )
    };
    AttestationResult::new(status, result)
}

impl AttestationResult {
    fn new(
        status: AVmAttestationStatus,
        result: *mut AVmAttestationResult,
    ) -> Result<AttestationResult, AttestationError> {
        match status {
            AVmAttestationStatus::ATTESTATION_ERROR_INVALID_CHALLENGE => {
                Err(AttestationError::InvalidChallenge)
            }
            AVmAttestationStatus::ATTESTATION_ERROR_ATTESTATION_FAILED => {
                Err(AttestationError::AttestationFailed)
            }
            AVmAttestationStatus::ATTESTATION_ERROR_UNSUPPORTED => {
                Err(AttestationError::AttestationUnsupported)
            }
            AVmAttestationStatus::ATTESTATION_OK => {
                let result = NonNull::new(result)
                    .expect("Attestation succeeded but the attestation result is null");
                Ok(AttestationResult { result })
            }
        }
    }

    fn as_const_ptr(&self) -> *const AVmAttestationResult {
        self.result.as_ptr().cast_const()
    }

    /// Returns the attested private key. This is the ECDSA P-256 private key corresponding to the
    /// public key described by the leaf certificate in the attested
    /// [certificate chain](AttestationResult::certificate_chain). It is a DER-encoded
    /// `ECPrivateKey` structure as specified in
    /// [RFC 5915 s3](https://datatracker.ietf.org/doc/html/rfc5915#section-3).
    ///
    /// Note: The [`sign_message`](AttestationResult::sign_message) method allows signing with the
    /// key without retrieving it.
    pub fn private_key(&self) -> Vec<u8> {
        let ptr = self.as_const_ptr();

        let size =
            // SAFETY: We own the `AVmAttestationResult` pointer, so it is valid. The function
            // writes no data since we pass a zero size, and null is explicitly allowed for the
            // destination in that case.
            unsafe { AVmAttestationResult_getPrivateKey(ptr, ptr::null_mut(), 0) };

        let mut private_key = vec![0u8; size];
        // SAFETY: We own the `AVmAttestationResult` pointer, so it is valid. The function only
        // writes within the bounds of `private_key`, which we just allocated so cannot be aliased.
        let size = unsafe {
            AVmAttestationResult_getPrivateKey(
                ptr,
                private_key.as_mut_ptr() as *mut c_void,
                private_key.len(),
            )
        };
        assert_eq!(size, private_key.len());
        private_key
    }

    /// Signs the given message using the attested private key. The signature uses ECDSA P-256; the
    /// message is first hashed with SHA-256 and then it is signed with the attested EC P-256
    /// [private key](AttestationResult::private_key).
    ///
    /// The signature is a DER-encoded `ECDSASignature`` structure as described in
    /// [RFC 6979](https://datatracker.ietf.org/doc/html/rfc6979).
    pub fn sign_message(&self, message: &[u8]) -> Vec<u8> {
        let ptr = self.as_const_ptr();

        // SAFETY: We own the `AVmAttestationResult` pointer, so it is valid. The function
        // writes no data since we pass a zero size, and null is explicitly allowed for the
        // destination in that case.
        let size = unsafe {
            AVmAttestationResult_sign(
                ptr,
                message.as_ptr() as *const c_void,
                message.len(),
                ptr::null_mut(),
                0,
            )
        };

        let mut signature = vec![0u8; size];
        // SAFETY: We own the `AVmAttestationResult` pointer, so it is valid. The function only
        // writes within the bounds of `signature`, which we just allocated so cannot be aliased.
        let size = unsafe {
            AVmAttestationResult_sign(
                ptr,
                message.as_ptr() as *const c_void,
                message.len(),
                signature.as_mut_ptr() as *mut c_void,
                signature.len(),
            )
        };
        assert!(size <= signature.len());
        signature.truncate(size);
        signature
    }

    /// Returns an iterator over the certificates forming the certificate chain for the VM, and its
    /// public key, obtained by the attestation process.
    ///
    /// The certificate chain consists of a sequence of DER-encoded X.509 certificates that form
    /// the attestation key's certificate chain. It starts with the leaf certificate covering the
    /// attested public key and ends with the root certificate.
    pub fn certificate_chain(&self) -> CertIterator {
        // SAFETY: We own the `AVmAttestationResult` pointer, so it is valid.
        let count = unsafe { AVmAttestationResult_getCertificateCount(self.as_const_ptr()) };

        CertIterator { result: self, count, current: 0 }
    }

    fn certificate(&self, index: usize) -> Vec<u8> {
        let ptr = self.as_const_ptr();

        let size =
            // SAFETY: We own the `AVmAttestationResult` pointer, so it is valid. The function
            // writes no data since we pass a zero size, and null is explicitly allowed for the
            // destination in that case. The function will panic if `index` is out of range (which
            // is safe).
            unsafe { AVmAttestationResult_getCertificateAt(ptr, index, ptr::null_mut(), 0) };

        let mut cert = vec![0u8; size];
        // SAFETY: We own the `AVmAttestationResult` pointer, so it is valid. The function only
        // writes within the bounds of `cert`, which we just allocated so cannot be aliased.
        let size = unsafe {
            AVmAttestationResult_getCertificateAt(
                ptr,
                index,
                cert.as_mut_ptr() as *mut c_void,
                cert.len(),
            )
        };
        assert_eq!(size, cert.len());
        cert
    }
}

/// An iterator over the DER-encoded X.509 certificates containin in an [`AttestationResult`].
/// See [`certificate_chain`](AttestationResult::certificate_chain) for more details.
pub struct CertIterator<'a> {
    result: &'a AttestationResult,
    count: usize,
    current: usize, // Invariant: current <= count
}

impl<'a> Iterator for CertIterator<'a> {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current < self.count {
            let cert = self.result.certificate(self.current);
            self.current += 1;
            Some(cert)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let size = self.count - self.current;
        (size, Some(size))
    }
}

impl<'a> ExactSizeIterator for CertIterator<'a> {}
impl<'a> FusedIterator for CertIterator<'a> {}
