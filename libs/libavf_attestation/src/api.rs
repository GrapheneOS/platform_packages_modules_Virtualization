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

//! This module contains the main API for the request processing module.

use crate::client_vm;
use crate::ops::AttestationOps;
use crate::rkp;
use alloc::vec::Vec;
use service_vm_comm::{Request, Response};

/// Processes a request and returns the corresponding response.
/// This function serves as the entry point for the request processing module.
pub fn process_request(request: Request, ops: &impl AttestationOps) -> Response {
    match request {
        Request::Reverse(v) => Response::Reverse(reverse(v)),
        Request::GenerateEcdsaP256KeyPair => rkp::generate_ecdsa_p256_key_pair(ops)
            .map_or_else(Response::Err, Response::GenerateEcdsaP256KeyPair),
        Request::GenerateCertificateRequest(p) => rkp::generate_certificate_request(p, ops)
            .map_or_else(Response::Err, Response::GenerateCertificateRequest),
        Request::RequestClientVmAttestation(p) => client_vm::request_attestation(p, ops)
            .map_or_else(Response::Err, Response::RequestClientVmAttestation),
    }
}

fn reverse(payload: Vec<u8>) -> Vec<u8> {
    payload.into_iter().rev().collect()
}
