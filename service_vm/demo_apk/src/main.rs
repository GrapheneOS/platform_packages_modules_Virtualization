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

use anyhow::{ensure, Context, Result};
use log::{error, info};
use std::panic;
use vm_payload::AttestationError;

vm_payload::main!(main);

/// Entry point of the Service VM client.
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

    let too_big_challenge = &[0u8; 66];
    let res = vm_payload::request_attestation(too_big_challenge);
    ensure!(res.is_err());
    let error = res.unwrap_err();
    ensure!(error == AttestationError::InvalidChallenge, "Unexpected error: {error:?}");
    info!("Error: {error}");

    // The data below is only a placeholder generated randomly with urandom
    let challenge = &[
        0x6c, 0xad, 0x52, 0x50, 0x15, 0xe7, 0xf4, 0x1d, 0xa5, 0x60, 0x7e, 0xd2, 0x7d, 0xf1, 0x51,
        0x67, 0xc3, 0x3e, 0x73, 0x9b, 0x30, 0xbd, 0x04, 0x20, 0x2e, 0xde, 0x3b, 0x1d, 0xc8, 0x07,
        0x11, 0x7b,
    ];
    let res = vm_payload::request_attestation(challenge).context("Unexpected attestation error")?;

    let cert_chain: Vec<_> = res.certificate_chain().collect();
    info!("Attestation result certificateChain = {:?}", cert_chain);

    let private_key = res.private_key();
    info!("Attestation result privateKey = {:?}", private_key);

    let message = b"Hello from Service VM client";
    info!("Signing message: {:?}", message);
    let signature = res.sign_message(message);
    info!("Signature: {:?}", signature);

    Ok(())
}
