// Copyright 2024, The Android Open Source Project
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

//! Main executable of VM attestation for end-to-end testing.

use anyhow::{anyhow, ensure, Result};
use avflog::LogResult;
use com_android_virt_vm_attestation_testservice::{
    aidl::com::android::virt::vm_attestation::testservice::IAttestationService::{
        AttestationStatus::AttestationStatus, BnAttestationService, IAttestationService,
        SigningResult::SigningResult, PORT,
    },
    binder::{self, unstable_api::AsNative, BinderFeatures, Interface, IntoBinderResult, Strong},
};
use log::{error, info};
use std::{
    ffi::{c_void, CStr},
    panic,
    ptr::{self, NonNull},
    result,
    sync::{Arc, Mutex},
};
use vm_payload_bindgen::{
    AIBinder, AVmAttestationResult, AVmAttestationResult_free,
    AVmAttestationResult_getCertificateAt, AVmAttestationResult_getCertificateCount,
    AVmAttestationResult_getPrivateKey, AVmAttestationResult_sign, AVmAttestationStatus,
    AVmAttestationStatus_toString, AVmPayload_notifyPayloadReady, AVmPayload_requestAttestation,
    AVmPayload_requestAttestationForTesting, AVmPayload_runVsockRpcServer,
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

    let mut service = AttestationService::new_binder().as_binder();
    let service = service.as_native_mut() as *mut AIBinder;
    let param = ptr::null_mut();
    // SAFETY: We hold a strong pointer, so the raw pointer remains valid. The bindgen AIBinder
    // is the same type as `sys::AIBinder`. It is safe for `on_ready` to be invoked at any time,
    // with any parameter.
    unsafe { AVmPayload_runVsockRpcServer(service, PORT.try_into()?, Some(on_ready), param) };
}

extern "C" fn on_ready(_param: *mut c_void) {
    // SAFETY: It is safe to call `AVmPayload_notifyPayloadReady` at any time.
    unsafe { AVmPayload_notifyPayloadReady() };
}

struct AttestationService {
    res: Arc<Mutex<Option<AttestationResult>>>,
}

impl Interface for AttestationService {}

impl AttestationService {
    fn new_binder() -> Strong<dyn IAttestationService> {
        let res = Arc::new(Mutex::new(None));
        BnAttestationService::new_binder(AttestationService { res }, BinderFeatures::default())
    }
}

impl IAttestationService for AttestationService {
    fn requestAttestationForTesting(&self) -> binder::Result<()> {
        const CHALLENGE: &[u8] = &[0xaa; 32];
        let res = AttestationResult::request_attestation_for_testing(CHALLENGE)
            .map_err(|e| anyhow!("Unexpected status: {:?}", status_to_cstr(e)))
            .with_log()
            .or_service_specific_exception(-1)?;
        *self.res.lock().unwrap() = Some(res);
        Ok(())
    }

    fn signWithAttestationKey(
        &self,
        challenge: &[u8],
        message: &[u8],
    ) -> binder::Result<SigningResult> {
        let res = match AttestationResult::request_attestation(challenge) {
            Ok(res) => res,
            Err(status) => {
                let status = to_attestation_status(status);
                return Ok(SigningResult { certificateChain: vec![], signature: vec![], status });
            }
        };
        let certificate_chain =
            res.certificate_chain().with_log().or_service_specific_exception(-1)?;
        let status = AttestationStatus::ATTESTATION_OK;
        let signature = res.sign(message).with_log().or_service_specific_exception(-1)?;
        Ok(SigningResult { certificateChain: certificate_chain, signature, status })
    }

    fn validateAttestationResult(&self) -> binder::Result<()> {
        // TODO(b/191073073): Returns the attestation result to the host for validation.
        self.res.lock().unwrap().as_ref().unwrap().log().or_service_specific_exception(-1)
    }
}

fn to_attestation_status(status: AVmAttestationStatus) -> AttestationStatus {
    match status {
        AVmAttestationStatus::ATTESTATION_OK => AttestationStatus::ATTESTATION_OK,
        AVmAttestationStatus::ATTESTATION_ERROR_INVALID_CHALLENGE => {
            AttestationStatus::ATTESTATION_ERROR_INVALID_CHALLENGE
        }
        AVmAttestationStatus::ATTESTATION_ERROR_ATTESTATION_FAILED => {
            AttestationStatus::ATTESTATION_ERROR_ATTESTATION_FAILED
        }
        AVmAttestationStatus::ATTESTATION_ERROR_UNSUPPORTED => {
            AttestationStatus::ATTESTATION_ERROR_UNSUPPORTED
        }
    }
}

#[derive(Debug)]
struct AttestationResult(NonNull<AVmAttestationResult>);

// Safety: `AttestationResult` is not `Send` because it contains a raw pointer to a C struct.
unsafe impl Send for AttestationResult {}

impl AttestationResult {
    fn request_attestation_for_testing(
        challenge: &[u8],
    ) -> result::Result<Self, AVmAttestationStatus> {
        let mut res: *mut AVmAttestationResult = ptr::null_mut();
        // SAFETY: It is safe as we only read the challenge within its bounds and the
        // function does not retain any reference to it.
        let status = unsafe {
            AVmPayload_requestAttestationForTesting(
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

    fn certificate_chain(&self) -> Result<Vec<u8>> {
        let num_certs = get_certificate_count(self.as_ref());
        let mut certs = Vec::new();
        for i in 0..num_certs {
            certs.extend(get_certificate_at(self.as_ref(), i)?.iter());
        }
        Ok(certs)
    }

    fn private_key(&self) -> Result<Box<[u8]>> {
        get_private_key(self.as_ref())
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        sign_with_attested_key(self.as_ref(), message)
    }

    fn log(&self) -> Result<()> {
        let cert_chain = self.certificate_chain()?;
        info!("Attestation result certificateChain = {:?}", cert_chain);

        let private_key = self.private_key()?;
        info!("Attestation result privateKey = {:?}", private_key);

        let message = b"Hello from Service VM client";
        info!("Signing message: {:?}", message);
        let signature = self.sign(message)?;
        info!("Signature: {:?}", signature);
        Ok(())
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

fn sign_with_attested_key(res: &AVmAttestationResult, message: &[u8]) -> Result<Vec<u8>> {
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
    ensure!(size <= signature.len());
    signature.truncate(size);
    Ok(signature)
}

fn status_to_cstr(status: AVmAttestationStatus) -> &'static CStr {
    // SAFETY: The function only reads the given enum status and returns a pointer to a
    // static string.
    let message = unsafe { AVmAttestationStatus_toString(status) };
    // SAFETY: The pointer returned by `AVmAttestationStatus_toString` is guaranteed to
    // point to a valid C String that lives forever.
    unsafe { CStr::from_ptr(message) }
}
