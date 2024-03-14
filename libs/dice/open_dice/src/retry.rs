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

//! This module implements a retry version for multiple DICE functions that
//! require preallocated output buffer. As the retry functions require
//! memory allocation on heap, currently we only expose these functions in
//! std environment.

use crate::bcc::{bcc_format_config_descriptor, bcc_main_flow, DiceConfigValues};
use crate::dice::{
    dice_main_flow, Cdi, CdiValues, DiceArtifacts, InputValues, CDI_SIZE, PRIVATE_KEY_SEED_SIZE,
};
use crate::error::{DiceError, Result};
use crate::ops::generate_certificate;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;
#[cfg(feature = "serde_derive")]
use serde_derive::{Deserialize, Serialize};

/// Artifacts stores a set of dice artifacts comprising CDI_ATTEST, CDI_SEAL,
/// and the BCC formatted attestation certificate chain.
/// As we align with the DICE standards today, this is the certificate chain
/// is also called DICE certificate chain.
#[derive(Debug)]
#[cfg_attr(feature = "serde_derive", derive(Serialize, Deserialize))]
pub struct OwnedDiceArtifacts {
    /// CDI Values.
    cdi_values: CdiValues,
    /// Boot Certificate Chain.
    bcc: Vec<u8>,
}

impl DiceArtifacts for OwnedDiceArtifacts {
    fn cdi_attest(&self) -> &[u8; CDI_SIZE] {
        &self.cdi_values.cdi_attest
    }

    fn cdi_seal(&self) -> &[u8; CDI_SIZE] {
        &self.cdi_values.cdi_seal
    }

    fn bcc(&self) -> Option<&[u8]> {
        Some(&self.bcc)
    }
}

/// Retries the given function with bigger measured buffer size.
fn retry_with_measured_buffer<F>(mut f: F) -> Result<Vec<u8>>
where
    F: FnMut(&mut Vec<u8>) -> Result<usize>,
{
    let mut buffer = Vec::new();
    match f(&mut buffer) {
        Err(DiceError::BufferTooSmall(actual_size)) => {
            buffer.resize(actual_size, 0);
            f(&mut buffer)?;
        }
        Err(e) => return Err(e),
        Ok(_) => {}
    };
    Ok(buffer)
}

/// Formats a configuration descriptor following the BCC's specification.
pub fn retry_bcc_format_config_descriptor(values: &DiceConfigValues) -> Result<Vec<u8>> {
    retry_with_measured_buffer(|buffer| bcc_format_config_descriptor(values, buffer))
}

/// Executes the main BCC flow.
///
/// Given a full set of input values along with the current BCC and CDI values,
/// computes the next CDI values and matching updated BCC.
pub fn retry_bcc_main_flow(
    current_cdi_attest: &Cdi,
    current_cdi_seal: &Cdi,
    bcc: &[u8],
    input_values: &InputValues,
) -> Result<OwnedDiceArtifacts> {
    let mut next_cdi_values = CdiValues::default();
    let next_bcc = retry_with_measured_buffer(|next_bcc| {
        bcc_main_flow(
            current_cdi_attest,
            current_cdi_seal,
            bcc,
            input_values,
            &mut next_cdi_values,
            next_bcc,
        )
    })?;
    Ok(OwnedDiceArtifacts { cdi_values: next_cdi_values, bcc: next_bcc })
}

/// Executes the main DICE flow.
///
/// Given a full set of input values and the current CDI values, computes the
/// next CDI values and a matching certificate.
pub fn retry_dice_main_flow(
    current_cdi_attest: &Cdi,
    current_cdi_seal: &Cdi,
    input_values: &InputValues,
) -> Result<(CdiValues, Vec<u8>)> {
    let mut next_cdi_values = CdiValues::default();
    let next_cdi_certificate = retry_with_measured_buffer(|next_cdi_certificate| {
        dice_main_flow(
            current_cdi_attest,
            current_cdi_seal,
            input_values,
            next_cdi_certificate,
            &mut next_cdi_values,
        )
    })?;
    Ok((next_cdi_values, next_cdi_certificate))
}

/// Generates an X.509 certificate from the given `subject_private_key_seed` and
/// `input_values`, and signed by `authority_private_key_seed`.
/// The subject private key seed is supplied here so the implementation can choose
/// between asymmetric mechanisms, for example ECDSA vs Ed25519.
/// Returns the generated certificate.
pub fn retry_generate_certificate(
    subject_private_key_seed: &[u8; PRIVATE_KEY_SEED_SIZE],
    authority_private_key_seed: &[u8; PRIVATE_KEY_SEED_SIZE],
    input_values: &InputValues,
) -> Result<Vec<u8>> {
    retry_with_measured_buffer(|certificate| {
        generate_certificate(
            subject_private_key_seed,
            authority_private_key_seed,
            input_values,
            certificate,
        )
    })
}
