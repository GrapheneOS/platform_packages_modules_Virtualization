// Copyright 2021, The Android Open Source Project
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

//! Android VirtualizationService

mod aidl;
mod atom;
mod maintenance;
mod remote_provisioning;
mod rkpvm;

use crate::aidl::{
    is_remote_provisioning_hal_declared, remove_temporary_dir, VirtualizationServiceInternal,
    TEMPORARY_DIRECTORY,
};
use android_logger::{Config, FilterBuilder};
use android_system_virtualizationmaintenance::aidl::android::system::virtualizationmaintenance;
use android_system_virtualizationservice_internal::aidl::android::system::virtualizationservice_internal;
use anyhow::{bail, Context, Error, Result};
use binder::{register_lazy_service, BinderFeatures, ProcessState, ThreadState};
use log::{error, info, LevelFilter};
use std::fs::{create_dir, read_dir};
use std::os::unix::raw::{pid_t, uid_t};
use std::path::Path;
use virtualizationmaintenance::IVirtualizationMaintenance::BnVirtualizationMaintenance;
use virtualizationservice_internal::IVirtualizationServiceInternal::BnVirtualizationServiceInternal;

const LOG_TAG: &str = "VirtualizationService";
pub(crate) const REMOTELY_PROVISIONED_COMPONENT_SERVICE_NAME: &str =
    "android.hardware.security.keymint.IRemotelyProvisionedComponent/avf";
const INTERNAL_SERVICE_NAME: &str = "android.system.virtualizationservice";
const MAINTENANCE_SERVICE_NAME: &str = "android.system.virtualizationmaintenance";

fn get_calling_pid() -> pid_t {
    ThreadState::get_calling_pid()
}

fn get_calling_uid() -> uid_t {
    ThreadState::get_calling_uid()
}

fn main() {
    if let Err(e) = try_main() {
        error!("failed with {e:?}");
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    android_logger::init_once(
        Config::default()
            .with_tag(LOG_TAG)
            .with_max_level(LevelFilter::Info)
            .with_log_buffer(android_logger::LogId::System)
            .with_filter(
                // Reduce logspam by silencing logs from the disk crate which don't provide much
                // information to us.
                FilterBuilder::new().parse("info,disk=off").build(),
            ),
    );

    clear_temporary_files().context("Failed to delete old temporary files")?;

    let common_dir_path = Path::new(TEMPORARY_DIRECTORY).join("common");
    create_dir(common_dir_path).context("Failed to create common directory")?;

    ProcessState::start_thread_pool();

    // One instance of `VirtualizationServiceInternal` implements both the internal interface
    // and (optionally) the maintenance interface.
    let service = VirtualizationServiceInternal::init();
    let internal_service =
        BnVirtualizationServiceInternal::new_binder(service.clone(), BinderFeatures::default());
    register(INTERNAL_SERVICE_NAME, internal_service)?;

    if is_remote_provisioning_hal_declared().unwrap_or(false) {
        // The IRemotelyProvisionedComponent service is only supposed to be triggered by rkpd for
        // RKP VM attestation.
        let remote_provisioning_service = remote_provisioning::new_binder();
        register(REMOTELY_PROVISIONED_COMPONENT_SERVICE_NAME, remote_provisioning_service)?;
    }

    if cfg!(llpvm_changes) {
        let maintenance_service =
            BnVirtualizationMaintenance::new_binder(service.clone(), BinderFeatures::default());
        register(MAINTENANCE_SERVICE_NAME, maintenance_service)?;
    }

    ProcessState::join_thread_pool();
    bail!("Thread pool unexpectedly ended");
}

fn register<T: binder::FromIBinder + ?Sized>(name: &str, service: binder::Strong<T>) -> Result<()> {
    register_lazy_service(name, service.as_binder())
        .with_context(|| format!("Failed to register {name}"))?;
    info!("Registered Binder service {name}.");
    Ok(())
}

/// Remove any files under `TEMPORARY_DIRECTORY`.
fn clear_temporary_files() -> Result<(), Error> {
    for dir_entry in read_dir(TEMPORARY_DIRECTORY)? {
        remove_temporary_dir(&dir_entry?.path())?
    }
    Ok(())
}
