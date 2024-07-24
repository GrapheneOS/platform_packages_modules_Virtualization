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

//! A tool to verify a CompOS signature. It starts a CompOS VM as part of this to retrieve the
//!  public key. The tool is intended to be run by odsign during boot.

use android_logger::LogId;
use anyhow::{anyhow, bail, Context, Result};
use binder::ProcessState;
use clap::{Parser, ValueEnum};
use compos_common::compos_client::{ComposClient, VmCpuTopology, VmParameters};
use compos_common::odrefresh::{
    CURRENT_ARTIFACTS_SUBDIR, ODREFRESH_OUTPUT_ROOT_DIR, PENDING_ARTIFACTS_SUBDIR,
    TEST_ARTIFACTS_SUBDIR,
};
use compos_common::{
    COMPOS_DATA_ROOT, CURRENT_INSTANCE_DIR, IDSIG_FILE, IDSIG_MANIFEST_APK_FILE,
    IDSIG_MANIFEST_EXT_APK_FILE, INSTANCE_ID_FILE, INSTANCE_IMAGE_FILE, TEST_INSTANCE_DIR,
};
use log::error;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::panic;
use std::path::Path;

const MAX_FILE_SIZE_BYTES: u64 = 100 * 1024;

#[derive(Parser)]
struct Args {
    /// Type of the VM instance
    #[clap(long, value_enum)]
    instance: Instance,

    /// Starts the VM in debug mode
    #[clap(long, action)]
    debug: bool,
}

#[derive(ValueEnum, Clone)]
enum Instance {
    Current,
    Pending,
    Test,
}

fn main() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("compos_verify")
            .with_max_level(log::LevelFilter::Info)
            .with_log_buffer(LogId::System), // Needed to log successfully early in boot
    );

    // Redirect panic messages to logcat.
    panic::set_hook(Box::new(|panic_info| {
        error!("{}", panic_info);
    }));

    if let Err(e) = try_main() {
        error!("{:?}", e);
        std::process::exit(1)
    }
}

fn try_main() -> Result<()> {
    let args = Args::parse();
    let (instance_dir, artifacts_dir) = match args.instance {
        Instance::Current => (CURRENT_INSTANCE_DIR, CURRENT_ARTIFACTS_SUBDIR),
        Instance::Pending => (CURRENT_INSTANCE_DIR, PENDING_ARTIFACTS_SUBDIR),
        Instance::Test => (TEST_INSTANCE_DIR, TEST_ARTIFACTS_SUBDIR),
    };

    let instance_dir = Path::new(COMPOS_DATA_ROOT).join(instance_dir);
    let artifacts_dir = Path::new(ODREFRESH_OUTPUT_ROOT_DIR).join(artifacts_dir);

    if !instance_dir.is_dir() {
        bail!("{:?} is not a directory", instance_dir);
    }

    let instance_id_file = instance_dir.join(INSTANCE_ID_FILE);
    let instance_image = instance_dir.join(INSTANCE_IMAGE_FILE);
    let idsig = instance_dir.join(IDSIG_FILE);
    let idsig_manifest_apk = instance_dir.join(IDSIG_MANIFEST_APK_FILE);
    let idsig_manifest_ext_apk = instance_dir.join(IDSIG_MANIFEST_EXT_APK_FILE);

    let instance_id: [u8; 64] = if cfg!(llpvm_changes) {
        fs::read(instance_id_file)?.try_into().map_err(|_| anyhow!("Failed to get instance_id"))?
    } else {
        [0u8; 64]
    };
    let instance_image = File::open(instance_image).context("Failed to open instance image")?;

    let info = artifacts_dir.join("compos.info");
    let signature = artifacts_dir.join("compos.info.signature");

    let info = read_small_file(&info).context("Failed to read compos.info")?;
    let signature = read_small_file(&signature).context("Failed to read compos.info signature")?;

    // We need to start the thread pool to be able to receive Binder callbacks
    ProcessState::start_thread_pool();

    let virtmgr = vmclient::VirtualizationService::new()?;
    let virtualization_service = virtmgr.connect()?;
    let vm_instance = ComposClient::start(
        &*virtualization_service,
        instance_id,
        instance_image,
        &idsig,
        &idsig_manifest_apk,
        &idsig_manifest_ext_apk,
        &VmParameters {
            name: String::from("ComposVerify"),
            cpu_topology: VmCpuTopology::OneCpu, // This VM runs very little work at boot
            debug_mode: args.debug,
            ..Default::default()
        },
    )?;

    let service = vm_instance.connect_service()?;
    let public_key = service.getPublicKey().context("Getting public key");

    vm_instance.shutdown(service);

    if !compos_verify_native::verify(&public_key?, &signature, &info) {
        bail!("Signature verification failed");
    }

    Ok(())
}

fn read_small_file(file: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(file)?;
    if file.metadata()?.len() > MAX_FILE_SIZE_BYTES {
        bail!("File is too big");
    }
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_args() {
        // Check that the command parsing has been configured in a valid way.
        Args::command().debug_assert();
    }
}
