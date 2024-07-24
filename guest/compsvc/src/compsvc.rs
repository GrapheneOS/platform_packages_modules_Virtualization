/*
 * Copyright (C) 2021 The Android Open Source Project
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

//! compsvc is a service to run compilation tasks in a PVM upon request. It is able to set up
//! file descriptors backed by authfs (via authfs_service) and pass the file descriptors to the
//! actual compiler.

use anyhow::{bail, Context, Result};
use log::{error, info};
use rustutils::system_properties;
use std::default::Default;
use std::fs::read_dir;
use std::iter::zip;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::artifact_signer::ArtifactSigner;
use crate::compilation::odrefresh;
use crate::compos_key;
use authfs_aidl_interface::aidl::com::android::virt::fs::IAuthFsService::{
    IAuthFsService, AUTHFS_SERVICE_SOCKET_NAME,
};
use binder::{
    BinderFeatures, ExceptionCode, Interface, IntoBinderResult, Result as BinderResult, Strong,
};
use compos_aidl_interface::aidl::com::android::compos::ICompOsService::{
    BnCompOsService, ICompOsService, OdrefreshArgs::OdrefreshArgs,
};
use compos_common::binder::to_binder_result;
use compos_common::odrefresh::{is_system_property_interesting, ODREFRESH_PATH};
use rpcbinder::RpcSession;

/// Constructs a binder object that implements ICompOsService.
pub fn new_binder() -> Result<Strong<dyn ICompOsService>> {
    let service = CompOsService {
        odrefresh_path: PathBuf::from(ODREFRESH_PATH),
        initialized: RwLock::new(None),
    };
    Ok(BnCompOsService::new_binder(service, BinderFeatures::default()))
}

struct CompOsService {
    odrefresh_path: PathBuf,

    /// A locked protected tri-state.
    ///  * None: uninitialized
    ///  * Some(true): initialized successfully
    ///  * Some(false): failed to initialize
    initialized: RwLock<Option<bool>>,
}

impl Interface for CompOsService {}

impl ICompOsService for CompOsService {
    fn initializeSystemProperties(&self, names: &[String], values: &[String]) -> BinderResult<()> {
        let mut initialized = self.initialized.write().unwrap();
        if initialized.is_some() {
            return Err(format!("Already initialized: {initialized:?}"))
                .or_binder_exception(ExceptionCode::ILLEGAL_STATE);
        }
        *initialized = Some(false);

        if names.len() != values.len() {
            return Err(format!(
                "Received inconsistent number of keys ({}) and values ({})",
                names.len(),
                values.len()
            ))
            .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
        }
        for (name, value) in zip(names, values) {
            if !is_system_property_interesting(name) {
                return Err(format!("Received invalid system property {name}"))
                    .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
            }
            let result = system_properties::write(name, value);
            if result.is_err() {
                error!("Failed to setprop {}", &name);
                return to_binder_result(result);
            }
        }
        *initialized = Some(true);
        Ok(())
    }

    fn odrefresh(&self, args: &OdrefreshArgs) -> BinderResult<i8> {
        let initialized = *self.initialized.read().unwrap();
        if !initialized.unwrap_or(false) {
            return Err("Service has not been initialized")
                .or_binder_exception(ExceptionCode::ILLEGAL_STATE);
        }

        to_binder_result(self.do_odrefresh(args))
    }

    fn getPublicKey(&self) -> BinderResult<Vec<u8>> {
        to_binder_result(compos_key::get_public_key())
    }

    fn getAttestationChain(&self) -> BinderResult<Vec<u8>> {
        to_binder_result(compos_key::get_attestation_chain())
    }

    fn quit(&self) -> BinderResult<()> {
        // When our process exits, Microdroid will shut down the VM.
        info!("Received quit request, exiting");
        std::process::exit(0);
    }
}

impl CompOsService {
    fn do_odrefresh(&self, args: &OdrefreshArgs) -> Result<i8> {
        log::debug!("Prepare to connect to {}", AUTHFS_SERVICE_SOCKET_NAME);
        let authfs_service: Strong<dyn IAuthFsService> = RpcSession::new()
            .setup_unix_domain_client(AUTHFS_SERVICE_SOCKET_NAME)
            .with_context(|| format!("Failed to connect to {}", AUTHFS_SERVICE_SOCKET_NAME))?;
        let exit_code = odrefresh(&self.odrefresh_path, args, authfs_service, |output_dir| {
            // authfs only shows us the files we created, so it's ok to just sign everything
            // under the output directory.
            let mut artifact_signer = ArtifactSigner::new(&output_dir);
            add_artifacts(&output_dir, &mut artifact_signer)?;

            artifact_signer.write_info_and_signature(&output_dir.join("compos.info"))
        })
        .context("odrefresh failed")?;
        Ok(exit_code as i8)
    }
}

fn add_artifacts(target_dir: &Path, artifact_signer: &mut ArtifactSigner) -> Result<()> {
    for entry in
        read_dir(target_dir).with_context(|| format!("Traversing {}", target_dir.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            add_artifacts(&entry.path(), artifact_signer)?;
        } else if file_type.is_file() {
            artifact_signer.add_artifact(&entry.path())?;
        } else {
            // authfs shouldn't create anything else, but just in case
            bail!("Unexpected file type in artifacts: {:?}", entry);
        }
    }
    Ok(())
}
