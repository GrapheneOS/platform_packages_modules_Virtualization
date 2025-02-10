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

//! Integration test for VMs on device.

use android_system_virtualizationservice::{
    aidl::android::system::virtualizationservice::{
        CpuOptions::CpuOptions, CpuOptions::CpuTopology::CpuTopology, DiskImage::DiskImage,
        VirtualMachineConfig::VirtualMachineConfig,
        VirtualMachineRawConfig::VirtualMachineRawConfig,
    },
    binder::{ParcelFileDescriptor, ProcessState},
};
use anyhow::anyhow;
use anyhow::Context;
use anyhow::Error;
use log::error;
use log::info;
use std::fs::read_to_string;
use std::fs::File;
use std::io::Write;
use std::process::Command;
use vmclient::VmInstance;

const VMBASE_EXAMPLE_KERNEL_PATH: &str = "vmbase_example_kernel.bin";
const TEST_DISK_IMAGE_PATH: &str = "test_disk.img";
const EMPTY_DISK_IMAGE_PATH: &str = "empty_disk.img";
const GOLDEN_DEVICE_TREE: &str = "./goldens/dt_dump_golden.dts";
const GOLDEN_DEVICE_TREE_PROTECTED: &str = "./goldens/dt_dump_protected_golden.dts";

/// Runs an unprotected VM and validates it against a golden device tree.
#[test]
fn test_device_tree_compat() -> Result<(), Error> {
    run_test(false, GOLDEN_DEVICE_TREE)
}

/// Runs a protected VM and validates it against a golden device tree.
#[test]
fn test_device_tree_protected_compat() -> Result<(), Error> {
    run_test(true, GOLDEN_DEVICE_TREE_PROTECTED)
}

fn run_test(protected: bool, golden_dt: &str) -> Result<(), Error> {
    let kernel = Some(open_payload(VMBASE_EXAMPLE_KERNEL_PATH)?);
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("backcompat")
            .with_max_level(log::LevelFilter::Debug),
    );

    // We need to start the thread pool for Binder to work properly, especially link_to_death.
    ProcessState::start_thread_pool();

    let virtmgr =
        vmclient::VirtualizationService::new().context("Failed to spawn VirtualizationService")?;
    let service = virtmgr.connect().context("Failed to connect to VirtualizationService")?;

    // Make file for test disk image.
    let mut test_image = File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(TEST_DISK_IMAGE_PATH)
        .with_context(|| format!("Failed to open test disk image {}", TEST_DISK_IMAGE_PATH))?;
    // Write 4 sectors worth of 4-byte numbers counting up.
    for i in 0u32..512 {
        test_image.write_all(&i.to_le_bytes())?;
    }
    let test_image = ParcelFileDescriptor::new(test_image);
    let disk_image = DiskImage { image: Some(test_image), writable: false, partitions: vec![] };

    // Make file for empty test disk image.
    let empty_image = File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(EMPTY_DISK_IMAGE_PATH)
        .with_context(|| format!("Failed to open empty disk image {}", EMPTY_DISK_IMAGE_PATH))?;
    let empty_image = ParcelFileDescriptor::new(empty_image);
    let empty_disk_image =
        DiskImage { image: Some(empty_image), writable: false, partitions: vec![] };

    let config = VirtualMachineConfig::RawConfig(VirtualMachineRawConfig {
        name: String::from("VmBaseTest"),
        kernel,
        disks: vec![disk_image, empty_disk_image],
        protectedVm: protected,
        memoryMib: 300,
        cpuOptions: CpuOptions { cpuTopology: CpuTopology::CpuCount(1) },
        platformVersion: "~1.0".to_string(),
        ..Default::default()
    });

    let dump_dt = File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open("dump_dt.dtb")
        .with_context(|| "Failed to open device tree dump file dump_dt.dtb")?;
    let vm = VmInstance::create(
        service.as_ref(),
        &config,
        None,
        /* consoleIn */ None,
        None,
        Some(dump_dt),
    )
    .context("Failed to create VM")?;
    vm.start(None).context("Failed to start VM")?;
    info!("Started example VM.");

    // Wait for VM to finish
    let _ = vm.wait_for_death();

    if !Command::new("./dtc_static")
        .arg("-I")
        .arg("dts")
        .arg("-O")
        .arg("dtb")
        .arg("-qqq")
        .arg("-f")
        .arg("-s")
        .arg("-o")
        .arg("dump_dt_golden.dtb")
        .arg(golden_dt)
        .output()?
        .status
        .success()
    {
        return Err(anyhow!("failed to execute dtc"));
    }
    let dtcompare_res = Command::new("./dtcompare")
        .arg("--dt1")
        .arg("dump_dt_golden.dtb")
        .arg("--dt2")
        .arg("dump_dt.dtb")
        .arg("--ignore-path-value")
        .arg("/chosen/kaslr-seed")
        .arg("--ignore-path-value")
        .arg("/chosen/rng-seed")
        // TODO: b/391420337 Investigate if bootargs may mutate VM
        .arg("--ignore-path-value")
        .arg("/chosen/bootargs")
        .arg("--ignore-path-value")
        .arg("/config/kernel-size")
        .arg("--ignore-path-value")
        .arg("/avf/untrusted/instance-id")
        .arg("--ignore-path-value")
        .arg("/chosen/linux,initrd-start")
        .arg("--ignore-path-value")
        .arg("/chosen/linux,initrd-end")
        .arg("--ignore-path-value")
        .arg("/avf/secretkeeper_public_key")
        .arg("--ignore-path")
        .arg("/avf/name")
        .output()
        .context("failed to execute dtcompare")?;
    if !dtcompare_res.status.success() {
        if !Command::new("./dtc_static")
            .arg("-I")
            .arg("dtb")
            .arg("-O")
            .arg("dts")
            .arg("-qqq")
            .arg("-f")
            .arg("-s")
            .arg("-o")
            .arg("dump_dt_failed.dts")
            .arg("dump_dt.dtb")
            .output()?
            .status
            .success()
        {
            return Err(anyhow!("failed to execute dtc"));
        }
        let dt2 = read_to_string("dump_dt_failed.dts")?;
        error!(
            "Device tree 2 does not match golden DT.\n
               Device Tree 2: {}",
            dt2
        );
        return Err(anyhow!(
            "stdout: {:?}\n stderr: {:?}",
            dtcompare_res.stdout,
            dtcompare_res.stderr
        ));
    }

    Ok(())
}

fn open_payload(path: &str) -> Result<ParcelFileDescriptor, Error> {
    let file = File::open(path).with_context(|| format!("Failed to open VM image {path}"))?;
    Ok(ParcelFileDescriptor::new(file))
}
