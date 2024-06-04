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

//! Support for starting CompOS in a VM and connecting to the service

use crate::timeouts::TIMEOUTS;
use crate::{
    get_vm_config_path, BUILD_MANIFEST_APK_PATH, BUILD_MANIFEST_SYSTEM_EXT_APK_PATH,
    COMPOS_APEX_ROOT, COMPOS_VSOCK_PORT,
};
use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    CpuTopology::CpuTopology,
    IVirtualizationService::IVirtualizationService,
    VirtualMachineAppConfig::{
        CustomConfig::CustomConfig, DebugLevel::DebugLevel, Payload::Payload,
        VirtualMachineAppConfig,
    },
    VirtualMachineConfig::VirtualMachineConfig,
};
use anyhow::{anyhow, bail, Context, Result};
use binder::{ParcelFileDescriptor, Strong};
use compos_aidl_interface::aidl::com::android::compos::ICompOsService::ICompOsService;
use glob::glob;
use log::{info, warn};
use platformproperties::hypervisorproperties;
use std::fs::File;
use std::path::{Path, PathBuf};
use vmclient::{DeathReason, ErrorCode, VmInstance, VmWaitError};

/// This owns an instance of the CompOS VM.
pub struct ComposClient(VmInstance);

/// CPU topology configuration for a virtual machine.
#[derive(Default, Debug, Clone)]
pub enum VmCpuTopology {
    /// Run VM with 1 vCPU only.
    #[default]
    OneCpu,
    /// Run VM vCPU topology matching that of the host.
    MatchHost,
}

/// Parameters to be used when creating a virtual machine instance.
#[derive(Default, Debug, Clone)]
pub struct VmParameters {
    /// The name of VM for identifying.
    pub name: String,
    /// Whether the VM should be debuggable.
    pub debug_mode: bool,
    /// CPU topology of the VM. Defaults to 1 vCPU.
    pub cpu_topology: VmCpuTopology,
    /// If present, overrides the amount of RAM to give the VM
    pub memory_mib: Option<i32>,
    /// Whether the VM prefers staged APEXes or activated ones (false; default)
    pub prefer_staged: bool,
}

impl ComposClient {
    /// Start a new CompOS VM instance using the specified instance image file and parameters.
    pub fn start(
        service: &dyn IVirtualizationService,
        instance_id: [u8; 64],
        instance_image: File,
        idsig: &Path,
        idsig_manifest_apk: &Path,
        idsig_manifest_ext_apk: &Path,
        parameters: &VmParameters,
    ) -> Result<Self> {
        let have_protected_vm =
            hypervisorproperties::hypervisor_protected_vm_supported()?.unwrap_or(false);
        if !have_protected_vm {
            bail!("Protected VM not supported, unable to start VM");
        }

        let instance_fd = ParcelFileDescriptor::new(instance_image);

        let apex_dir = Path::new(COMPOS_APEX_ROOT);

        let config_apk = locate_config_apk(apex_dir)?;
        let apk_fd = File::open(config_apk).context("Failed to open config APK file")?;
        let apk_fd = ParcelFileDescriptor::new(apk_fd);
        let idsig_fd = prepare_idsig(service, &apk_fd, idsig)?;

        let manifest_apk_fd = File::open(BUILD_MANIFEST_APK_PATH)
            .context("Failed to open build manifest APK file")?;
        let manifest_apk_fd = ParcelFileDescriptor::new(manifest_apk_fd);
        let idsig_manifest_apk_fd = prepare_idsig(service, &manifest_apk_fd, idsig_manifest_apk)?;

        // Prepare a few things based on whether /system_ext exists, including:
        // 1. generate the additional idsig FD for the APK from /system_ext, then pass to VS
        // 2. select the correct VM config json
        let (extra_idsigs, has_system_ext) =
            if let Ok(manifest_ext_apk_fd) = File::open(BUILD_MANIFEST_SYSTEM_EXT_APK_PATH) {
                // Optional idsig in /system_ext is found, so prepare additionally.
                let manifest_ext_apk_fd = ParcelFileDescriptor::new(manifest_ext_apk_fd);
                let idsig_manifest_ext_apk_fd =
                    prepare_idsig(service, &manifest_ext_apk_fd, idsig_manifest_ext_apk)?;

                (vec![idsig_manifest_apk_fd, idsig_manifest_ext_apk_fd], true)
            } else {
                (vec![idsig_manifest_apk_fd], false)
            };
        let config_path = get_vm_config_path(has_system_ext, parameters.prefer_staged);

        let debug_level = if parameters.debug_mode { DebugLevel::FULL } else { DebugLevel::NONE };

        let cpu_topology = match parameters.cpu_topology {
            VmCpuTopology::OneCpu => CpuTopology::ONE_CPU,
            VmCpuTopology::MatchHost => CpuTopology::MATCH_HOST,
        };

        // The CompOS VM doesn't need to be updatable (by design it should run exactly twice,
        // with the same APKs and APEXes each time). And having it so causes some interesting
        // circular dependencies when run at boot time by odsign: b/331417880.
        let custom_config = Some(CustomConfig { wantUpdatable: false, ..Default::default() });

        let config = VirtualMachineConfig::AppConfig(VirtualMachineAppConfig {
            name: parameters.name.clone(),
            apk: Some(apk_fd),
            idsig: Some(idsig_fd),
            instanceId: instance_id,
            instanceImage: Some(instance_fd),
            payload: Payload::ConfigPath(config_path),
            debugLevel: debug_level,
            extraIdsigs: extra_idsigs,
            protectedVm: true,
            memoryMib: parameters.memory_mib.unwrap_or(0), // 0 means use the default
            cpuTopology: cpu_topology,
            customConfig: custom_config,
            ..Default::default()
        });

        // Let logs go to logcat.
        let (console_fd, log_fd) = (None, None);
        let callback = Box::new(Callback {});
        let instance = VmInstance::create(
            service,
            &config,
            console_fd,
            /* console_in_fd */ None,
            log_fd,
            Some(callback),
        )
        .context("Failed to create VM")?;

        instance.start()?;

        let ready = instance.wait_until_ready(TIMEOUTS.vm_max_time_to_ready);
        if ready == Err(VmWaitError::Finished) && debug_level != DebugLevel::NONE {
            // The payload has (unexpectedly) finished, but the VM is still running. Give it
            // some time to shutdown to maximize our chances of getting useful logs.
            if let Some(death_reason) =
                instance.wait_for_death_with_timeout(TIMEOUTS.vm_max_time_to_exit)
            {
                bail!("VM died during startup - reason {:?}", death_reason);
            }
        }
        ready?;

        Ok(Self(instance))
    }

    /// Create and return an RPC Binder connection to the Comp OS service in the VM.
    pub fn connect_service(&self) -> Result<Strong<dyn ICompOsService>> {
        self.0.connect_service(COMPOS_VSOCK_PORT).context("Connecting to CompOS service")
    }

    /// Shut down the VM cleanly, by sending a quit request to the service, giving time for any
    /// relevant logs to be written.
    pub fn shutdown(self, service: Strong<dyn ICompOsService>) {
        info!("Requesting CompOS VM to shutdown");
        let _ignored = service.quit(); // If this fails, the VM is probably dying anyway
        self.wait_for_shutdown();
    }

    /// Wait for the instance to shut down. If it fails to shutdown within a reasonable time the
    /// instance is dropped, which forcibly terminates it.
    /// This should only be called when the instance has been requested to quit, or we believe that
    /// it is already in the process of exiting due to some failure.
    fn wait_for_shutdown(self) {
        let death_reason = self.0.wait_for_death_with_timeout(TIMEOUTS.vm_max_time_to_exit);
        match death_reason {
            Some(DeathReason::Shutdown) => info!("VM has exited normally"),
            Some(reason) => warn!("VM died with reason {:?}", reason),
            None => warn!("VM failed to exit, dropping"),
        }
    }
}

fn locate_config_apk(apex_dir: &Path) -> Result<PathBuf> {
    // Our config APK will be in a directory under app, but the name of the directory is at the
    // discretion of the build system. So just look in each sub-directory until we find it.
    // (In practice there will be exactly one directory, so this shouldn't take long.)
    let app_glob = apex_dir.join("app").join("**").join("CompOSPayloadApp*.apk");
    let mut entries: Vec<PathBuf> =
        glob(app_glob.to_str().ok_or_else(|| anyhow!("Invalid path: {}", app_glob.display()))?)
            .context("failed to glob")?
            .filter_map(|e| e.ok())
            .collect();
    if entries.len() > 1 {
        bail!("Found more than one apk matching {}", app_glob.display());
    }
    match entries.pop() {
        Some(path) => Ok(path),
        None => Err(anyhow!("No apks match {}", app_glob.display())),
    }
}

fn prepare_idsig(
    service: &dyn IVirtualizationService,
    apk_fd: &ParcelFileDescriptor,
    idsig_path: &Path,
) -> Result<ParcelFileDescriptor> {
    if !idsig_path.exists() {
        // Prepare idsig file via VirtualizationService
        let idsig_file = File::create(idsig_path).context("Failed to create idsig file")?;
        let idsig_fd = ParcelFileDescriptor::new(idsig_file);
        service
            .createOrUpdateIdsigFile(apk_fd, &idsig_fd)
            .context("Failed to update idsig file")?;
    }

    // Open idsig as read-only
    let idsig_file = File::open(idsig_path).context("Failed to open idsig file")?;
    let idsig_fd = ParcelFileDescriptor::new(idsig_file);
    Ok(idsig_fd)
}

struct Callback {}
impl vmclient::VmCallback for Callback {
    fn on_payload_started(&self, cid: i32) {
        log::info!("VM payload started, cid = {}", cid);
    }

    fn on_payload_ready(&self, cid: i32) {
        log::info!("VM payload ready, cid = {}", cid);
    }

    fn on_payload_finished(&self, cid: i32, exit_code: i32) {
        log::warn!("VM payload finished, cid = {}, exit code = {}", cid, exit_code);
    }

    fn on_error(&self, cid: i32, error_code: ErrorCode, message: &str) {
        log::warn!("VM error, cid = {}, error code = {:?}, message = {}", cid, error_code, message);
    }

    fn on_died(&self, cid: i32, death_reason: DeathReason) {
        log::warn!("VM died, cid = {}, reason = {:?}", cid, death_reason);
    }
}
