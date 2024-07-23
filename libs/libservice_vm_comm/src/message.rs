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

//! This module contains the requests and responses definitions exchanged
//! between the host and the service VM.

use alloc::vec::Vec;
use core::fmt;
use log::error;
use serde::{Deserialize, Serialize};

type MacedPublicKey = Vec<u8>;

/// The main request type to be sent to the service VM.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServiceVmRequest {
    /// A request to be processed by the service VM.
    ///
    /// Each request has a corresponding response item.
    Process(Request),

    /// Shuts down the service VM. No response is expected from it.
    Shutdown,
}

/// Represents a process request to be sent to the service VM.
///
/// Each request has a corresponding response item.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    /// Reverse the order of the bytes in the provided byte array.
    /// Currently this is only used for testing.
    Reverse(Vec<u8>),

    /// Generates a new ECDSA P-256 key pair that can be attested by the remote
    /// server.
    GenerateEcdsaP256KeyPair,

    /// Creates a certificate signing request to be sent to the
    /// provisioning server.
    GenerateCertificateRequest(GenerateCertificateRequestParams),

    /// Requests the service VM to attest the client VM and issue a certificate
    /// if the attestation succeeds.
    RequestClientVmAttestation(ClientVmAttestationParams),
}

impl Request {
    /// Returns the name of the request.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Reverse(_) => "Reverse",
            Self::GenerateEcdsaP256KeyPair => "GenerateEcdsaP256KeyPair",
            Self::GenerateCertificateRequest(_) => "GenerateCertificateRequest",
            Self::RequestClientVmAttestation(_) => "RequestClientVmAttestation",
        }
    }
}

/// Represents the params passed to `Request::RequestClientVmAttestation`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientVmAttestationParams {
    /// The CBOR-encoded CSR signed by the CDI_Leaf_Priv of the client VM's DICE chain
    /// and the private key to be attested.
    /// See client_vm_csr.cddl for the definition of the CSR.
    pub csr: Vec<u8>,

    /// The key blob retrieved from RKPD by virtualizationservice.
    pub remotely_provisioned_key_blob: Vec<u8>,

    /// The leaf certificate of the certificate chain retrieved from RKPD by
    /// virtualizationservice.
    ///
    /// This certificate is a DER-encoded X.509 certificate that includes the remotely
    /// provisioned public key.
    pub remotely_provisioned_cert: Vec<u8>,
}

/// Represents a response to a request sent to the service VM.
///
/// Each response corresponds to a specific request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    /// Reverse the order of the bytes in the provided byte array.
    Reverse(Vec<u8>),

    /// Returns the new ECDSA P-256 key pair.
    GenerateEcdsaP256KeyPair(EcdsaP256KeyPair),

    /// Returns a CBOR Certificate Signing Request (Csr) serialized into a byte array.
    GenerateCertificateRequest(Vec<u8>),

    /// Returns a certificate covering the public key to be attested in the provided CSR.
    /// The certificate is signed by the remotely provisioned private key and also
    /// includes an extension that describes the attested client VM.
    RequestClientVmAttestation(Vec<u8>),

    /// Encountered an error during the request processing.
    Err(RequestProcessingError),
}

impl Response {
    /// Returns the name of the response.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Reverse(_) => "Reverse",
            Self::GenerateEcdsaP256KeyPair(_) => "GenerateEcdsaP256KeyPair",
            Self::GenerateCertificateRequest(_) => "GenerateCertificateRequest",
            Self::RequestClientVmAttestation(_) => "RequestClientVmAttestation",
            Self::Err(_) => "Err",
        }
    }
}

/// Errors related to request processing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestProcessingError {
    /// An error happened during the interaction with BoringSSL.
    BoringSslError(bssl_avf_error::Error),

    /// An error happened during the interaction with coset.
    CosetError,

    /// An unexpected internal error occurred.
    InternalError,

    /// Any key to sign lacks a valid MAC. Maps to `STATUS_INVALID_MAC`.
    InvalidMac,

    /// No payload found in a key to sign.
    KeyToSignHasEmptyPayload,

    /// An error happened when serializing to/from a `Value`.
    CborValueError,

    /// The DICE chain of the service VM is missing.
    MissingDiceChain,

    /// Failed to decrypt the remotely provisioned key blob.
    FailedToDecryptKeyBlob,

    /// The requested operation has not been implemented.
    OperationUnimplemented,

    /// An error happened during the DER encoding/decoding.
    DerError,

    /// The DICE chain from the client VM is invalid.
    InvalidDiceChain,

    /// Cannot find the vendor hash tree root digest in the device tree.
    NoVendorHashTreeRootDigestInDT,

    /// The vendor partition loaded by the client VM is invalid.
    InvalidVendorPartition,
}

impl fmt::Display for RequestProcessingError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::BoringSslError(e) => {
                write!(f, "An error happened during the interaction with BoringSSL: {e}")
            }
            Self::CosetError => write!(f, "Encountered an error with coset"),
            Self::InternalError => write!(f, "An unexpected internal error occurred"),
            Self::InvalidMac => write!(f, "A key to sign lacks a valid MAC."),
            Self::KeyToSignHasEmptyPayload => write!(f, "No payload found in a key to sign."),
            Self::CborValueError => {
                write!(f, "An error happened when serializing to/from a CBOR Value.")
            }
            Self::MissingDiceChain => write!(f, "The DICE chain of the service VM is missing"),
            Self::FailedToDecryptKeyBlob => {
                write!(f, "Failed to decrypt the remotely provisioned key blob")
            }
            Self::OperationUnimplemented => {
                write!(f, "The requested operation has not been implemented")
            }
            Self::DerError => {
                write!(f, "An error happened during the DER encoding/decoding")
            }
            Self::InvalidDiceChain => {
                write!(f, "The DICE chain from the client VM is invalid")
            }
            Self::NoVendorHashTreeRootDigestInDT => {
                write!(f, "Cannot find the vendor hash tree root digest in the device tree")
            }
            Self::InvalidVendorPartition => {
                write!(f, "The vendor partition loaded by the client VM is invalid")
            }
        }
    }
}

impl From<bssl_avf_error::Error> for RequestProcessingError {
    fn from(e: bssl_avf_error::Error) -> Self {
        Self::BoringSslError(e)
    }
}

impl From<coset::CoseError> for RequestProcessingError {
    fn from(e: coset::CoseError) -> Self {
        error!("Coset error: {e}");
        Self::CosetError
    }
}

impl From<ciborium::value::Error> for RequestProcessingError {
    fn from(e: ciborium::value::Error) -> Self {
        error!("CborValueError: {e}");
        Self::CborValueError
    }
}

#[cfg(not(feature = "std"))]
impl From<der::Error> for RequestProcessingError {
    fn from(e: der::Error) -> Self {
        error!("DER encoding/decoding error: {e}");
        Self::DerError
    }
}

/// Represents the params passed to GenerateCertificateRequest
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenerateCertificateRequestParams {
    /// Contains the set of keys to certify.
    pub keys_to_sign: Vec<MacedPublicKey>,

    /// challenge contains a byte strong from the provisioning server which will be
    /// included in the signed data of the CSR structure.
    /// The supported sizes is between 0 and 64 bytes, inclusive.
    pub challenge: Vec<u8>,
}

/// Represents an ECDSA P-256 key pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcdsaP256KeyPair {
    /// Contains a CBOR-encoded public key specified in:
    ///
    /// hardware/interfaces/security/rkp/aidl/android/hardware/security/keymint/MacedPublicKey.aidl
    pub maced_public_key: MacedPublicKey,

    /// Contains a handle to the private key.
    pub key_blob: Vec<u8>,
}
