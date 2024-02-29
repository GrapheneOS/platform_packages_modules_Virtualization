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

//! Main executable of Service VM client for manual testing.

use anyhow::{anyhow, ensure, Result};
use log::{error, info};
use std::{
    ffi::{c_void, CStr},
    panic,
    ptr::{self, NonNull},
    result,
};
use vm_payload_bindgen::{
    AVmAttestationResult, AVmAttestationResult_free, AVmAttestationResult_getCertificateAt,
    AVmAttestationResult_getCertificateCount, AVmAttestationResult_getPrivateKey,
    AVmAttestationResult_sign, AVmAttestationStatus, AVmAttestationStatus_toString,
    AVmPayload_requestAttestation,
};

/// Entry point of the Service VM client.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn AVmPayload_main() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("service_vm_client")
            .with_max_level(log::LevelFilter::Debug),
    );
    // Redirect panic messages to logcat.
    panic::set_hook(Box::new(|panic_info| {
        error!("{}", panic_info);
    }));
    if let Err(e) = try_main() {
        error!("failed with {:?}", e);
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    info!("Welcome to Service VM Client!");

    let too_big_challenge = &[0u8; 66];
    let res = AttestationResult::request_attestation(too_big_challenge);
    ensure!(res.is_err());
    let status = res.unwrap_err();
    ensure!(
        status == AVmAttestationStatus::ATTESTATION_ERROR_INVALID_CHALLENGE,
        "Unexpected status: {:?}",
        status
    );
    info!("Status: {:?}", status_to_cstr(status));

    // The data below is only a placeholder generated randomly with urandom
    let challenge = &[
        0x6c, 0xad, 0x52, 0x50, 0x15, 0xe7, 0xf4, 0x1d, 0xa5, 0x60, 0x7e, 0xd2, 0x7d, 0xf1, 0x51,
        0x67, 0xc3, 0x3e, 0x73, 0x9b, 0x30, 0xbd, 0x04, 0x20, 0x2e, 0xde, 0x3b, 0x1d, 0xc8, 0x07,
        0x11, 0x7b,
    ];
    let res = AttestationResult::request_attestation(challenge)
        .map_err(|e| anyhow!("Unexpected status: {:?}", status_to_cstr(e)))?;

    let cert_chain = res.certificate_chain()?;
    info!("Attestation result certificateChain = {:?}", cert_chain);

    let private_key = res.private_key()?;
    info!("Attestation result privateKey = {:?}", private_key);

    let message = b"Hello from Service VM client";
    info!("Signing message: {:?}", message);
    let signature = res.sign(message)?;
    info!("Signature: {:?}", signature);

    Ok(())
}

#[derive(Debug)]
struct AttestationResult(NonNull<AVmAttestationResult>);

impl AttestationResult {
    fn request_attestation(challenge: &[u8]) -> result::Result<Self, AVmAttestationStatus> {
        let mut res: *mut AVmAttestationResult = ptr::null_mut();
        // SAFETY: It is safe as we only read the challenge within its bounds and the
        // function does not retain any reference to it.
        let status = unsafe {
            AVmPayload_requestAttestation(
                challenge.as_ptr() as *const c_void,
                challenge.len(),
                &mut res,
            )
        };
        if status == AVmAttestationStatus::ATTESTATION_OK {
            info!("Attestation succeeds. Status: {:?}", status_to_cstr(status));
            let res = NonNull::new(res).expect("The attestation result is null");
            Ok(Self(res))
        } else {
            Err(status)
        }
    }

    fn certificate_chain(&self) -> Result<Vec<Box<[u8]>>> {
        let num_certs = get_certificate_count(self.as_ref());
        let mut certs = Vec::with_capacity(num_certs);
        for i in 0..num_certs {
            certs.push(get_certificate_at(self.as_ref(), i)?);
        }
        Ok(certs)
    }

    fn private_key(&self) -> Result<Box<[u8]>> {
        get_private_key(self.as_ref())
    }

    fn sign(&self, message: &[u8]) -> Result<Box<[u8]>> {
        sign_with_attested_key(self.as_ref(), message)
    }
}

impl AsRef<AVmAttestationResult> for AttestationResult {
    fn as_ref(&self) -> &AVmAttestationResult {
        // SAFETY: This field is private, and only populated with a successful call to
        // `AVmPayload_requestAttestation`.
        unsafe { self.0.as_ref() }
    }
}

impl Drop for AttestationResult {
    fn drop(&mut self) {
        // SAFETY: This field is private, and only populated with a successful call to
        // `AVmPayload_requestAttestation`, and not freed elsewhere.
        unsafe { AVmAttestationResult_free(self.0.as_ptr()) };
    }
}

fn get_certificate_count(res: &AVmAttestationResult) -> usize {
    // SAFETY: The result is returned by `AVmPayload_requestAttestation` and should be valid
    // before getting freed.
    unsafe { AVmAttestationResult_getCertificateCount(res) }
}

fn get_certificate_at(res: &AVmAttestationResult, index: usize) -> Result<Box<[u8]>> {
    let size =
        // SAFETY: The result is returned by `AVmPayload_requestAttestation` and should be valid
        // before getting freed.
        unsafe { AVmAttestationResult_getCertificateAt(res, index, ptr::null_mut(), 0) };
    let mut cert = vec![0u8; size];
    // SAFETY: The result is returned by `AVmPayload_requestAttestation` and should be valid
    // before getting freed. This function only writes within the bounds of `cert`.
    // And `cert` cannot overlap `res` because we just allocated it.
    let size = unsafe {
        AVmAttestationResult_getCertificateAt(
            res,
            index,
            cert.as_mut_ptr() as *mut c_void,
            cert.len(),
        )
    };
    ensure!(size == cert.len());
    Ok(cert.into_boxed_slice())
}

fn get_private_key(res: &AVmAttestationResult) -> Result<Box<[u8]>> {
    let size =
        // SAFETY: The result is returned by `AVmPayload_requestAttestation` and should be valid
        // before getting freed.
        unsafe { AVmAttestationResult_getPrivateKey(res, ptr::null_mut(), 0) };
    let mut private_key = vec![0u8; size];
    // SAFETY: The result is returned by `AVmPayload_requestAttestation` and should be valid
    // before getting freed. This function only writes within the bounds of `private_key`.
    // And `private_key` cannot overlap `res` because we just allocated it.
    let size = unsafe {
        AVmAttestationResult_getPrivateKey(
            res,
            private_key.as_mut_ptr() as *mut c_void,
            private_key.len(),
        )
    };
    ensure!(size == private_key.len());
    Ok(private_key.into_boxed_slice())
}

fn sign_with_attested_key(res: &AVmAttestationResult, message: &[u8]) -> Result<Box<[u8]>> {
    // SAFETY: The result is returned by `AVmPayload_requestAttestation` and should be valid
    // before getting freed.
    let size = unsafe {
        AVmAttestationResult_sign(
            res,
            message.as_ptr() as *const c_void,
            message.len(),
            ptr::null_mut(),
            0,
        )
    };
    let mut signature = vec![0u8; size];
    // SAFETY: The result is returned by `AVmPayload_requestAttestation` and should be valid
    // before getting freed. This function only writes within the bounds of `signature`.
    // And `signature` cannot overlap `res` because we just allocated it.
    let size = unsafe {
        AVmAttestationResult_sign(
            res,
            message.as_ptr() as *const c_void,
            message.len(),
            signature.as_mut_ptr() as *mut c_void,
            signature.len(),
        )
    };
    ensure!(size == signature.len());
    Ok(signature.into_boxed_slice())
}

fn status_to_cstr(status: AVmAttestationStatus) -> &'static CStr {
    // SAFETY: The function only reads the given enum status and returns a pointer to a
    // static string.
    let message = unsafe { AVmAttestationStatus_toString(status) };
    // SAFETY: The pointer returned by `AVmAttestationStatus_toString` is guaranteed to
    // point to a valid C String that lives forever.
    unsafe { CStr::from_ptr(message) }
}
