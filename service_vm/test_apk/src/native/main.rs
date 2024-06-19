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

use anyhow::Result;
use avflog::LogResult;
use com_android_virt_vm_attestation_testservice::{
    aidl::com::android::virt::vm_attestation::testservice::IAttestationService::{
        AttestationStatus::AttestationStatus, BnAttestationService, IAttestationService,
        SigningResult::SigningResult, PORT,
    },
    binder::{self, BinderFeatures, Interface, IntoBinderResult, Strong},
};
use log::{error, info};
use std::{
    panic,
    sync::{Arc, Mutex},
};
use vm_payload::{AttestationError, AttestationResult};

vm_payload::main!(main);

// Entry point of the Service VM client.
fn main() {
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

    vm_payload::run_single_vsock_service(AttestationService::new_binder(), PORT.try_into()?)
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

#[allow(non_snake_case)]
impl IAttestationService for AttestationService {
    fn requestAttestationForTesting(&self) -> binder::Result<()> {
        const CHALLENGE: &[u8] = &[0xaa; 32];
        let res = vm_payload::restricted::request_attestation_for_testing(CHALLENGE)
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
        let res: AttestationResult = match vm_payload::request_attestation(challenge) {
            Ok(res) => res,
            Err(e) => {
                let status = to_attestation_status(e);
                return Ok(SigningResult { certificateChain: vec![], signature: vec![], status });
            }
        };

        let certificate_chain: Vec<u8> = res.certificate_chain().flatten().collect();
        let status = AttestationStatus::OK;
        let signature = res.sign_message(message);

        Ok(SigningResult { certificateChain: certificate_chain, signature, status })
    }

    fn validateAttestationResult(&self) -> binder::Result<()> {
        // TODO(b/191073073): Returns the attestation result to the host for validation.
        log(self.res.lock().unwrap().as_ref().unwrap());
        Ok(())
    }
}

fn log(res: &AttestationResult) {
    for (i, cert) in res.certificate_chain().enumerate() {
        info!("Attestation result certificate {i} = {cert:?}");
    }

    let private_key = res.private_key();
    info!("Attestation result privateKey = {private_key:?}");

    let message = b"Hello from Service VM client";
    info!("Signing message: {message:?}");
    let signature = res.sign_message(message);
    info!("Signature: {signature:?}");
}

fn to_attestation_status(e: AttestationError) -> AttestationStatus {
    match e {
        AttestationError::InvalidChallenge => AttestationStatus::ERROR_INVALID_CHALLENGE,
        AttestationError::AttestationFailed => AttestationStatus::ERROR_ATTESTATION_FAILED,
        AttestationError::AttestationUnsupported => AttestationStatus::ERROR_UNSUPPORTED,
    }
}
