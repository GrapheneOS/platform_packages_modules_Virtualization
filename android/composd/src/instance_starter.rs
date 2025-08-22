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

use crate::wrappers::compos_common_injection;
#[cfg(not(test))]
use {crate::wrappers::binder::LazyServiceGuard, compos_wrappers::paths};
#[cfg(test)]
use {
    crate::wrappers::binder::MockLazyServiceGuard as LazyServiceGuard,
    compos_wrappers_with_mocks::mock_paths as paths,
};

use compos_common_injection::{
    compos_client::{CompOsService, CompOsType, VmParameters},
    COMPOS_DATA_ROOT, IDSIG_FILE, IDSIG_MANIFEST_APK_FILE, IDSIG_MANIFEST_EXT_APK_FILE,
    INSTANCE_ID_FILE, INSTANCE_IMAGE_FILE,
};

#[cfg(not(test))]
use compos_common_injection::compos_client::ComposClient;
#[cfg(test)]
use compos_common_injection::compos_client::MockComposClient as ComposClient;

use anyhow::{anyhow, Context, Result};
use binder::ParcelFileDescriptor;
use log::info;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

pub struct CompOsInstance {
    service: CompOsService,
    #[allow(dead_code)] // Keeps VirtualizationService & the VM alive
    vm_instance: ComposClient,
    #[allow(dead_code)] // Keeps composd process alive
    lazy_service_guard: LazyServiceGuard,
    // Keep this alive as long as we are
    instance_tracker: Arc<()>,
}

impl CompOsInstance {
    #[cfg(test)]
    pub fn new_for_test(
        vm_instance: ComposClient,
        service: CompOsService,
        lazy_service_guard: LazyServiceGuard,
    ) -> Self {
        Self { vm_instance, service, lazy_service_guard, instance_tracker: Default::default() }
    }
    pub fn get_service(&self) -> CompOsService {
        self.service.clone()
    }

    pub fn get_service_ref(&self) -> &CompOsService {
        &self.service
    }
    /// Returns an Arc that this instance holds a strong reference to as long as it exists. This
    /// can be used to determine when the instance has been dropped.
    pub fn get_instance_tracker(&self) -> &Arc<()> {
        &self.instance_tracker
    }

    /// Attempt to shut down the VM cleanly, giving time for any relevant logs to be written.
    pub fn shutdown(self) -> LazyServiceGuard {
        self.vm_instance.shutdown(&self.service);
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

#[cfg_attr(test, mockall::automock, allow(dead_code))]
impl InstanceStarter {
    pub fn new(instance_name: &str, vm_parameters: VmParameters) -> Self {
        let instance_root = paths::root_rebase(COMPOS_DATA_ROOT).join(instance_name);
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

        // For VM's with an OdRefresh service retrieve the attestation chain as
        // a BCC and save it in the instance directory.
        if let CompOsService::OdRefresh(ref s) = instance.get_service_ref() {
            let bcc = s
                .getAttestationChain()
                .context("Getting attestation chain from CompOS OdRefresh")?;
            fs::write(self.instance_root.join("bcc"), bcc).context("Writing BCC")?;
        }
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
        let service = match &self.vm_parameters.compos_type {
            CompOsType::OdRefresh => CompOsService::OdRefresh(
                vm_instance.connect_service().context("Connecting to CompOS OdRefresh")?,
            ),
            CompOsType::Dex2Oat => CompOsService::Dex2Oat(
                vm_instance.connect_service().context("Connecting to CompOS Dex2Oat")?,
            ),
        };
        Ok(CompOsInstance {
            vm_instance,
            service,
            lazy_service_guard: LazyServiceGuard::new(),
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

#[cfg(test)]
mod test {
    use super::InstanceStarter;
    use crate::{
        test_util::{file, parcel},
        wrappers,
    };
    use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
        IVirtualizationService::MockIVirtualizationService, PartitionType::PartitionType,
    };
    use anyhow::Error;
    use binder::{BinderFeatures, ParcelFileDescriptor as ParcelFd};
    use compos_aidl_interface::aidl::com::android::compos::ICompOsService::{
        BnCompOsService, MockICompOsService,
    };
    use compos_common_with_mocks::{
        binder::to_binder_result,
        compos_client::{CompOsService, CompOsType, MockComposClient, VmCpuTopology, VmParameters},
    };
    use compos_wrappers_with_mocks::mock_paths;
    use mockall::predicate::{always as any, eq, function as func, gt};
    use once_cell::sync::Lazy;
    use std::fs;
    use tempfile::{tempdir, TempDir};

    #[test]
    pub fn odrefresh_instance() {
        static ROOT_DIR: Lazy<TempDir> = Lazy::new(|| tempdir().unwrap());
        let root_rebase_ctx = mock_paths::root_rebase_context();
        root_rebase_ctx
            .expect()
            .withf(|frag: &str| frag.starts_with("/"))
            .returning(|frag: &str| ROOT_DIR.path().join(frag.strip_prefix("/").unwrap_or(frag)));

        let expected_vm_params = VmParameters {
            name: "test".to_string(),
            base_os: "microdroid".to_string(),
            cpu_topology: VmCpuTopology::MatchHost,
            memory_mib: Some(600),
            prefer_staged: false,
            compos_type: CompOsType::OdRefresh,
            debug_mode: false,
        };
        const INSTANCE_NAME: &str = "INSTANCE_NAME";
        const IMAGE_CONTENT: &[u8] = b"ODREFRESH_INSTANCE_IMAGE";
        const INSTANCE_ID: &[u8; 64] =
            b"ODREFRESH_INSTANCE_ID???????????????????????????????????????????";
        const DEFAULT_BCC: &[u8] = b"DEFAULT_BCC";

        let virt_svc = {
            let mut mock = MockIVirtualizationService::new();
            mock.expect_initializeWritablePartition()
                .with(
                    /* imageFd :&ParcelFd */
                    func(|image_fd: &ParcelFd| parcel::is_rw(image_fd)),
                    /* sizeBytes */ gt(0),
                    eq(PartitionType::ANDROID_VM_INSTANCE),
                )
                .return_once(|image_fd: &ParcelFd, _, _| {
                    to_binder_result(parcel::write(image_fd, IMAGE_CONTENT))
                });
            mock.expect_allocateInstanceId().return_once(|| Ok(*INSTANCE_ID));
            mock
        };

        let mut lazy_service_guard_mock = wrappers::binder::MockLazyServiceGuard::default();
        let compos_client_mock = {
            let compsvc_binder_mock = {
                let mut mock = MockICompOsService::new();
                mock.expect_getAttestationChain().returning(|| {
                    to_binder_result::<std::vec::Vec<u8>, Error>(Ok(DEFAULT_BCC.to_vec()))
                });
                BnCompOsService::new_binder(mock, BinderFeatures::default())
            };
            let mut mock = MockComposClient::new();
            mock.expect_connect_service().return_once(move || Ok(compsvc_binder_mock));
            mock.expect_shutdown().withf(|s| matches!(s, CompOsService::OdRefresh(_))).return_once(
                |s| {
                    if let CompOsService::OdRefresh(od) = s {
                        let _ = od.quit();
                    }
                },
            );
            lazy_service_guard_mock.expect_drop().times(1).return_const(());
            mock
        };

        let lazy_service_guard_new_ctx = wrappers::binder::MockLazyServiceGuard::new_context();

        let _compos_client_start_ctx = {
            lazy_service_guard_new_ctx.expect().return_once(|| lazy_service_guard_mock);
            let ctx = MockComposClient::start_context();
            ctx.expect()
                .with(
                    /* service: &dyn IVirtualizationService */ any(),
                    /* instance_id */ eq(INSTANCE_ID),
                    /* instance_image: &File */
                    func(|file: &fs::File| file::contents_equals(file, IMAGE_CONTENT)),
                    /* idsig path: &Path */ any(),
                    /* idsig_manifest_apk path: &Path */ any(),
                    /* idsig_manifest_ext_apk: &Path */ any(),
                    /* parameters: &VmParameters */
                    eq(expected_vm_params.clone()),
                )
                .return_once(move |_, _, _, _, _, _, _| Ok(compos_client_mock));
            ctx
        };
        let instance_starter = InstanceStarter::new(INSTANCE_NAME, expected_vm_params);
        let _instance = instance_starter.start_new_instance(&virt_svc);
    }
}
