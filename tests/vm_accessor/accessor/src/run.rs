// Copyright 2024, The Android Open Source Project
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

//! Command to run a VM.

use android_system_virtualizationservice::aidl::android::system::virtualizationservice::{
    IVirtualizationService::IVirtualizationService,
    PartitionType::PartitionType,
    VirtualMachineAppConfig::{DebugLevel::DebugLevel, Payload::Payload, VirtualMachineAppConfig},
    VirtualMachineConfig::VirtualMachineConfig,
    VirtualMachinePayloadConfig::VirtualMachinePayloadConfig,
};
use anyhow::{bail, Context, Error};
use binder::{ParcelFileDescriptor, Strong};
use glob::glob;
use log::{error, info};
use rand::{distributions::Alphanumeric, Rng};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::thread;
use vmclient::{ErrorCode, VmInstance};
use vmconfig::open_parcel_file;

// These are private contract between IAccessor impl and VM service.
const PAYLOAD_BINARY_NAME: &str = "libaccessor_vm_payload.so";
const VM_OS_NAME: &str = "microdroid";

const INSTANCE_FILE_SIZE: u64 = 10 * 1024 * 1024;

fn get_service() -> Result<Strong<dyn IVirtualizationService>, Error> {
    let virtmgr =
        vmclient::VirtualizationService::new().context("Failed to spawn VirtualizationService")?;
    virtmgr.connect().context("Failed to connect to VirtualizationService")
}

fn find_vm_apk_path() -> Result<PathBuf, Error> {
    const GLOB_PATTERN: &str = "/apex/com.android.virt.accessor_demo/app/**/AccessorVmApp*.apk";
    let mut entries: Vec<PathBuf> =
        glob(GLOB_PATTERN).context("failed to glob")?.filter_map(|e| e.ok()).collect();
    if entries.len() > 1 {
        bail!("Found more than one apk matching {}", GLOB_PATTERN);
    }
    if let Some(path) = entries.pop() {
        info!("Found accessor apk at {path:?}");
        Ok(path)
    } else {
        bail!("No apks match {}", GLOB_PATTERN)
    }
}

fn create_work_dir() -> Result<PathBuf, Error> {
    let s: String =
        rand::thread_rng().sample_iter(&Alphanumeric).take(17).map(char::from).collect();
    let work_dir = PathBuf::from("/data/local/tmp/microdroid").join(s);
    info!("creating work dir {}", work_dir.display());
    fs::create_dir_all(&work_dir).context("failed to mkdir")?;
    Ok(work_dir)
}

/// Run a VM with Microdroid
pub fn run_vm() -> Result<VmInstance, Error> {
    let service = get_service()?;

    let apk = File::open(find_vm_apk_path()?).context("Failed to open APK file")?;
    let apk_fd = ParcelFileDescriptor::new(apk);

    let work_dir = create_work_dir()?;
    info!("work dir: {}", work_dir.display());

    let idsig =
        File::create_new(work_dir.join("apk.idsig")).context("Failed to create idsig file")?;
    let idsig_fd = ParcelFileDescriptor::new(idsig);
    service.createOrUpdateIdsigFile(&apk_fd, &idsig_fd)?;

    let instance_img_path = work_dir.join("instance.img");
    let instance_img =
        File::create_new(&instance_img_path).context("Failed to create instance.img file")?;
    service.initializeWritablePartition(
        &ParcelFileDescriptor::new(instance_img),
        INSTANCE_FILE_SIZE.try_into()?,
        PartitionType::ANDROID_VM_INSTANCE,
    )?;
    info!("created instance image at: {instance_img_path:?}");

    let instance_id = if cfg!(llpvm_changes) {
        let id = service.allocateInstanceId().context("Failed to allocate instance_id")?;
        fs::write(work_dir.join("instance_id"), id)?;
        id
    } else {
        // if llpvm feature flag is disabled, instance_id is not used.
        [0u8; 64]
    };

    let payload = Payload::PayloadConfig(VirtualMachinePayloadConfig {
        payloadBinaryName: PAYLOAD_BINARY_NAME.to_owned(),
        extraApks: Default::default(),
    });

    let vm_config = VirtualMachineConfig::AppConfig(VirtualMachineAppConfig {
        name: String::from("AccessorVm"),
        apk: apk_fd.into(),
        idsig: idsig_fd.into(),
        extraIdsigs: Default::default(),
        instanceImage: open_parcel_file(&instance_img_path, true /* writable */)?.into(),
        instanceId: instance_id,
        payload,
        osName: VM_OS_NAME.to_owned(),
        debugLevel: DebugLevel::FULL,
        ..Default::default()
    });

    info!("creating VM");
    let vm = VmInstance::create(
        service.as_ref(),
        &vm_config,
        Some(android_log_fd()?), /* console_out */
        None,                    /* console_in */
        Some(android_log_fd()?), /* log */
        Some(Box::new(Callback {})),
    )
    .context("Failed to create VM")?;
    vm.start().context("Failed to start VM")?;

    info!("started IAccessor VM with CID {}", vm.cid());

    Ok(vm)
}

struct Callback {}

impl vmclient::VmCallback for Callback {
    fn on_payload_started(&self, _cid: i32) {
        info!("payload started");
    }

    fn on_payload_ready(&self, _cid: i32) {
        info!("payload is ready");
    }

    fn on_payload_finished(&self, _cid: i32, exit_code: i32) {
        info!("payload finished with exit code {}", exit_code);
    }

    fn on_error(&self, _cid: i32, error_code: ErrorCode, message: &str) {
        error!("VM encountered an error: code={:?}, message={}", error_code, message);
    }
}

/// This function is only exposed for testing.
/// Production code prefer not expose logs from VM.
fn android_log_fd() -> io::Result<File> {
    let (reader_fd, writer_fd) = nix::unistd::pipe()?;

    let reader = File::from(reader_fd);
    let writer = File::from(writer_fd);

    thread::spawn(|| {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(l) => info!("{}", l),
                Err(e) => {
                    error!("Failed to read line from VM: {e:?}");
                    break;
                }
            }
        }
    });
    Ok(writer)
}
