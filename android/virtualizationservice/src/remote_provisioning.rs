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

//! IRemotelyProvisionedComponent HAL implementation.

use crate::rkpvm;
use android_hardware_security_rkp::aidl::android::hardware::security::keymint::{
    DeviceInfo::DeviceInfo,
    IRemotelyProvisionedComponent::{
        BnRemotelyProvisionedComponent, IRemotelyProvisionedComponent, STATUS_FAILED,
        STATUS_INVALID_MAC, STATUS_REMOVED,
    },
    MacedPublicKey::MacedPublicKey,
    ProtectedData::ProtectedData,
    RpcHardwareInfo::{RpcHardwareInfo, CURVE_NONE, MIN_SUPPORTED_NUM_KEYS_IN_CSR},
};
use anyhow::Context;
use avflog::LogResult;
use binder::{
    BinderFeatures, ExceptionCode, Interface, IntoBinderResult, Result as BinderResult, Status,
    Strong,
};
use hypervisor_props::is_protected_vm_supported;
use rustutils::system_properties;
use service_vm_comm::{RequestProcessingError, Response};

/// Constructs a binder object that implements `IRemotelyProvisionedComponent`.
pub(crate) fn new_binder() -> Strong<dyn IRemotelyProvisionedComponent> {
    BnRemotelyProvisionedComponent::new_binder(
        AvfRemotelyProvisionedComponent {},
        BinderFeatures::default(),
    )
}

struct AvfRemotelyProvisionedComponent {}

impl Interface for AvfRemotelyProvisionedComponent {}

#[allow(non_snake_case)]
impl IRemotelyProvisionedComponent for AvfRemotelyProvisionedComponent {
    fn getHardwareInfo(&self) -> BinderResult<RpcHardwareInfo> {
        check_remote_attestation_is_supported()?;

        Ok(RpcHardwareInfo {
            versionNumber: 3,
            rpcAuthorName: String::from("Android Virtualization Framework"),
            supportedEekCurve: CURVE_NONE,
            uniqueId: Some(String::from("AVF Remote Provisioning 1")),
            supportedNumKeysInCsr: MIN_SUPPORTED_NUM_KEYS_IN_CSR,
        })
    }

    fn generateEcdsaP256KeyPair(
        &self,
        testMode: bool,
        macedPublicKey: &mut MacedPublicKey,
    ) -> BinderResult<Vec<u8>> {
        check_remote_attestation_is_supported()?;

        if testMode {
            return Err(Status::new_service_specific_error_str(
                STATUS_REMOVED,
                Some("generateEcdsaP256KeyPair does not support test mode in IRPC v3+ HAL."),
            ))
            .with_log();
        }
        let res = rkpvm::generate_ecdsa_p256_key_pair()
            .context("Failed to generate ECDSA P-256 key pair")
            .with_log()
            .or_service_specific_exception(STATUS_FAILED)?;
        match res {
            Response::GenerateEcdsaP256KeyPair(key_pair) => {
                macedPublicKey.macedKey = key_pair.maced_public_key;
                Ok(key_pair.key_blob)
            }
            _ => Err(to_service_specific_error(res)),
        }
        .with_log()
    }

    fn generateCertificateRequest(
        &self,
        _testMode: bool,
        _keysToSign: &[MacedPublicKey],
        _endpointEncryptionCertChain: &[u8],
        _challenge: &[u8],
        _deviceInfo: &mut DeviceInfo,
        _protectedData: &mut ProtectedData,
    ) -> BinderResult<Vec<u8>> {
        Err(Status::new_service_specific_error_str(
            STATUS_REMOVED,
            Some("This method was deprecated in v3 of the interface."),
        ))
        .with_log()
    }

    fn generateCertificateRequestV2(
        &self,
        keysToSign: &[MacedPublicKey],
        challenge: &[u8],
    ) -> BinderResult<Vec<u8>> {
        check_remote_attestation_is_supported()?;

        const MAX_CHALLENGE_SIZE: usize = 64;
        if challenge.len() > MAX_CHALLENGE_SIZE {
            let message = format!(
                "Challenge is too big. Actual: {:?}. Maximum: {:?}.",
                challenge.len(),
                MAX_CHALLENGE_SIZE
            );
            return Err(Status::new_service_specific_error_str(STATUS_FAILED, Some(message)))
                .with_log();
        }
        let res = rkpvm::generate_certificate_request(keysToSign, challenge)
            .context("Failed to generate certificate request")
            .with_log()
            .or_service_specific_exception(STATUS_FAILED)?;
        match res {
            Response::GenerateCertificateRequest(res) => Ok(res),
            _ => Err(to_service_specific_error(res)),
        }
        .with_log()
    }
}

pub(crate) fn check_remote_attestation_is_supported() -> BinderResult<()> {
    if !is_protected_vm_supported().unwrap_or(false) {
        return Err(Status::new_exception_str(
            ExceptionCode::UNSUPPORTED_OPERATION,
            Some("Protected VM support is missing for this operation"),
        ))
        .with_log();
    }
    if !is_remote_attestation_supported() {
        return Err(Status::new_exception_str(
            ExceptionCode::UNSUPPORTED_OPERATION,
            Some("Remote attestation is disabled"),
        ))
        .with_log();
    }
    Ok(())
}

pub(crate) fn is_remote_attestation_supported() -> bool {
    // Remote attestation is enabled by default.
    system_properties::read_bool("avf.remote_attestation.enabled", true).unwrap_or(true)
}

pub(crate) fn to_service_specific_error(response: Response) -> Status {
    match response {
        Response::Err(e) => match e {
            RequestProcessingError::InvalidMac => {
                Status::new_service_specific_error_str(STATUS_INVALID_MAC, Some(format!("{e}")))
            }
            _ => Status::new_service_specific_error_str(
                STATUS_FAILED,
                Some(format!("Failed to process request: {e}.")),
            ),
        },
        other => Status::new_service_specific_error_str(
            STATUS_FAILED,
            Some(format!("Incorrect response type: {other:?}")),
        ),
    }
}
