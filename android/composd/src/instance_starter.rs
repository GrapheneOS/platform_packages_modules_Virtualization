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

//! Responsible for validating and starting an existing instance of the CompOS VM, or creating and
//! starting a new instance if necessary.

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    IVirtualizationService::IVirtualizationService, PartitionType::PartitionType,
};
use anyhow::{anyhow, Context, Result};
use binder::{LazyServiceGuard, ParcelFileDescriptor, Strong};
use compos_aidl_interface::aidl::com::android::compos::ICompOsService::ICompOsService;
use compos_common::compos_client::{ComposClient, VmParameters};
use compos_common::{
    COMPOS_DATA_ROOT, IDSIG_FILE, IDSIG_MANIFEST_APK_FILE, IDSIG_MANIFEST_EXT_APK_FILE,
    INSTANCE_ID_FILE, INSTANCE_IMAGE_FILE,
};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct CompOsInstance {
    service: Strong<dyn ICompOsService>,
    #[allow(dead_code)] // Keeps VirtualizationService & the VM alive
    vm_instance: ComposClient,
    #[allow(dead_code)] // Keeps composd process alive
    lazy_service_guard: LazyServiceGuard,
    // Keep this alive as long as we are
    instance_tracker: Arc<()>,
}

impl CompOsInstance {
    pub fn get_service(&self) -> Strong<dyn ICompOsService> {
        self.service.clone()
    }

    /// Returns an Arc that this instance holds a strong reference to as long as it exists. This
    /// can be used to determine when the instance has been dropped.
    pub fn get_instance_tracker(&self) -> &Arc<()> {
        &self.instance_tracker
    }

    /// Attempt to shut down the VM cleanly, giving time for any relevant logs to be written.
    pub fn shutdown(self) -> LazyServiceGuard {
        self.vm_instance.shutdown(self.service);
        // Return the guard to the caller, since we might be terminated at any point after it is
        // dropped, and there might still be things to do.
        self.lazy_service_guard
    }
}

pub struct InstanceStarter {
    instance_name: String,
    instance_root: PathBuf,
    instance_id_file: PathBuf,
    instance_image: PathBuf,
    idsig: PathBuf,
    idsig_manifest_apk: PathBuf,
    idsig_manifest_ext_apk: PathBuf,
    vm_parameters: VmParameters,
}

impl InstanceStarter {
    pub fn new(instance_name: &str, vm_parameters: VmParameters) -> Self {
        let instance_root = Path::new(COMPOS_DATA_ROOT).join(instance_name);
        let instance_root_path = instance_root.as_path();
        let instance_id_file = instance_root_path.join(INSTANCE_ID_FILE);
        let instance_image = instance_root_path.join(INSTANCE_IMAGE_FILE);
        let idsig = instance_root_path.join(IDSIG_FILE);
        let idsig_manifest_apk = instance_root_path.join(IDSIG_MANIFEST_APK_FILE);
        let idsig_manifest_ext_apk = instance_root_path.join(IDSIG_MANIFEST_EXT_APK_FILE);
        Self {
            instance_name: instance_name.to_owned(),
            instance_root,
            instance_id_file,
            instance_image,
            idsig,
            idsig_manifest_apk,
            idsig_manifest_ext_apk,
            vm_parameters,
        }
    }

    pub fn start_new_instance(
        &self,
        virtualization_service: &dyn IVirtualizationService,
    ) -> Result<CompOsInstance> {
        info!("Creating {} CompOs instance", self.instance_name);

        fs::create_dir_all(&self.instance_root)?;

        // Overwrite any existing instance - it's unlikely to be valid with the current set
        // of APEXes, and finding out it isn't is much more expensive than creating a new one.
        self.create_instance_image(virtualization_service)?;
        // TODO(b/294177871): Ping VS to delete the old instance's secret.
        if cfg!(llpvm_changes) {
            self.allocate_instance_id(virtualization_service)?;
        }
        // Delete existing idsig files. Ignore error in case idsig doesn't exist.
        let _ignored1 = fs::remove_file(&self.idsig);
        let _ignored2 = fs::remove_file(&self.idsig_manifest_apk);
        let _ignored3 = fs::remove_file(&self.idsig_manifest_ext_apk);

        let instance = self.start_vm(virtualization_service)?;

        // Retrieve the VM's attestation chain as a BCC and save it in the instance directory.
        let bcc = instance.service.getAttestationChain().context("Getting attestation chain")?;
        fs::write(self.instance_root.join("bcc"), bcc).context("Writing BCC")?;

        Ok(instance)
    }

    fn start_vm(
        &self,
        virtualization_service: &dyn IVirtualizationService,
    ) -> Result<CompOsInstance> {
        let instance_id: [u8; 64] = if cfg!(llpvm_changes) {
            fs::read(&self.instance_id_file)?
                .try_into()
                .map_err(|_| anyhow!("Failed to get instance_id"))?
        } else {
            [0u8; 64]
        };

        let instance_image = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.instance_image)
            .context("Failed to open instance image")?;
        let vm_instance = ComposClient::start(
            virtualization_service,
            instance_id,
            instance_image,
            &self.idsig,
            &self.idsig_manifest_apk,
            &self.idsig_manifest_ext_apk,
            &self.vm_parameters,
        )
        .context("Starting VM")?;
        let service = vm_instance.connect_service().context("Connecting to CompOS")?;
        Ok(CompOsInstance {
            vm_instance,
            service,
            lazy_service_guard: Default::default(),
            instance_tracker: Default::default(),
        })
    }

    fn create_instance_image(
        &self,
        virtualization_service: &dyn IVirtualizationService,
    ) -> Result<()> {
        let instance_image = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&self.instance_image)
            .context("Creating instance image file")?;
        let instance_image = ParcelFileDescriptor::new(instance_image);
        // TODO: Where does this number come from?
        let size = 10 * 1024 * 1024;
        virtualization_service
            .initializeWritablePartition(&instance_image, size, PartitionType::ANDROID_VM_INSTANCE)
            .context("Writing instance image file")?;
        Ok(())
    }

    fn allocate_instance_id(
        &self,
        virtualization_service: &dyn IVirtualizationService,
    ) -> Result<()> {
        let id = virtualization_service.allocateInstanceId().context("Allocating Instance Id")?;
        fs::write(&self.instance_id_file, id)?;
        Ok(())
    }
}
