// Copyright 2022, The Android Open Source Project
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

//! Integration test for VM bootloader.

use android_system_virtualizationservice::{
    aidl::android::system::virtualizationservice::{
        CpuTopology::CpuTopology, DiskImage::DiskImage, VirtualMachineConfig::VirtualMachineConfig,
        VirtualMachineRawConfig::VirtualMachineRawConfig,
    },
    binder::{ParcelFileDescriptor, ProcessState},
};
use anyhow::{Context, Error};
use log::info;
use std::{
    collections::{HashSet, VecDeque},
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    panic, thread,
};
use vmclient::{DeathReason, VmInstance};

const VMBASE_EXAMPLE_KERNEL_PATH: &str = "vmbase_example_kernel.bin";
const VMBASE_EXAMPLE_BIOS_PATH: &str = "vmbase_example_bios.bin";
const TEST_DISK_IMAGE_PATH: &str = "test_disk.img";
const EMPTY_DISK_IMAGE_PATH: &str = "empty_disk.img";

/// Runs the vmbase_example VM as an unprotected VM kernel via VirtualizationService.
#[test]
fn test_run_example_kernel_vm() -> Result<(), Error> {
    run_test(Some(open_payload(VMBASE_EXAMPLE_KERNEL_PATH)?), None)
}

/// Runs the vmbase_example VM as an unprotected VM BIOS via VirtualizationService.
#[test]
fn test_run_example_bios_vm() -> Result<(), Error> {
    run_test(None, Some(open_payload(VMBASE_EXAMPLE_BIOS_PATH)?))
}

fn run_test(
    kernel: Option<ParcelFileDescriptor>,
    bootloader: Option<ParcelFileDescriptor>,
) -> Result<(), Error> {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("vmbase")
            .with_max_level(log::LevelFilter::Debug),
    );

    // Redirect panic messages to logcat.
    panic::set_hook(Box::new(|panic_info| {
        log::error!("{}", panic_info);
    }));

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
        initrd: None,
        params: None,
        bootloader,
        disks: vec![disk_image, empty_disk_image],
        protectedVm: false,
        memoryMib: 300,
        cpuTopology: CpuTopology::ONE_CPU,
        platformVersion: "~1.0".to_string(),
        gdbPort: 0, // no gdb
        ..Default::default()
    });
    let (handle, console) = android_log_fd()?;
    let (mut log_reader, log_writer) = pipe()?;
    let vm = VmInstance::create(
        service.as_ref(),
        &config,
        Some(console),
        /* consoleIn */ None,
        Some(log_writer),
        None,
    )
    .context("Failed to create VM")?;
    vm.start().context("Failed to start VM")?;
    info!("Started example VM.");

    // Wait for VM to finish, and check that it shut down cleanly.
    let death_reason = vm.wait_for_death();
    assert_eq!(death_reason, DeathReason::Shutdown);
    handle.join().unwrap();

    // Check that the expected string was written to the log VirtIO console device.
    let expected = "Hello VirtIO console\n";
    let mut log_output = String::new();
    assert_eq!(log_reader.read_to_string(&mut log_output)?, expected.len());
    assert_eq!(log_output, expected);

    Ok(())
}

fn android_log_fd() -> Result<(thread::JoinHandle<()>, File), io::Error> {
    let (reader, writer) = pipe()?;
    let handle = thread::spawn(|| VmLogProcessor::new(reader).run().unwrap());
    Ok((handle, writer))
}

fn pipe() -> io::Result<(File, File)> {
    let (reader_fd, writer_fd) = nix::unistd::pipe()?;
    Ok((reader_fd.into(), writer_fd.into()))
}

fn open_payload(path: &str) -> Result<ParcelFileDescriptor, Error> {
    let file = File::open(path).with_context(|| format!("Failed to open VM image {path}"))?;
    Ok(ParcelFileDescriptor::new(file))
}

struct VmLogProcessor {
    reader: Option<File>,
    expected: VecDeque<String>,
    unexpected: HashSet<String>,
    had_unexpected: bool,
}

impl VmLogProcessor {
    fn messages() -> (VecDeque<String>, HashSet<String>) {
        let mut expected = VecDeque::new();
        let mut unexpected = HashSet::new();
        for log_lvl in ["[ERROR]", "[WARN]", "[INFO]", "[DEBUG]"] {
            expected.push_back(format!("{log_lvl} Unsuppressed message"));
            unexpected.insert(format!("{log_lvl} Suppressed message"));
        }
        (expected, unexpected)
    }

    fn new(reader: File) -> Self {
        let (expected, unexpected) = Self::messages();
        Self { reader: Some(reader), expected, unexpected, had_unexpected: false }
    }

    fn verify(&mut self, msg: &str) {
        if self.expected.front() == Some(&msg.to_owned()) {
            self.expected.pop_front();
        }
        if !self.had_unexpected && self.unexpected.contains(msg) {
            self.had_unexpected = true;
        }
    }

    fn run(mut self) -> Result<(), &'static str> {
        for line in BufReader::new(self.reader.take().unwrap()).lines() {
            let msg = line.unwrap();
            info!("{msg}");
            self.verify(&msg);
        }
        if !self.expected.is_empty() {
            Err("missing expected log message")
        } else if self.had_unexpected {
            Err("unexpected log message")
        } else {
            Ok(())
        }
    }
}
