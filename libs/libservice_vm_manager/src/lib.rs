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

//! This module contains the functions to start, stop and communicate with the
//! Service VM.

use android_system_virtualizationservice::{
    aidl::android::system::virtualizationservice::{
        CpuTopology::CpuTopology, DiskImage::DiskImage,
        IVirtualizationService::IVirtualizationService, Partition::Partition,
        PartitionType::PartitionType, VirtualMachineConfig::VirtualMachineConfig,
        VirtualMachineRawConfig::VirtualMachineRawConfig,
    },
    binder::ParcelFileDescriptor,
};
use anyhow::{anyhow, ensure, Context, Result};
use log::{info, warn};
use service_vm_comm::{Request, Response, ServiceVmRequest, VmType};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::Duration;
use vmclient::{DeathReason, VmInstance};
use vsock::{VsockListener, VsockStream, VMADDR_CID_HOST};

/// Size of virtual memory allocated to the Service VM.
pub const VM_MEMORY_MB: i32 = 6;

const VIRT_DATA_DIR: &str = "/data/misc/apexdata/com.android.virt";
const RIALTO_PATH: &str = "/apex/com.android.virt/etc/rialto.bin";
const INSTANCE_IMG_NAME: &str = "service_vm_instance.img";
const INSTANCE_ID_FILENAME: &str = "service_vm_instance_id";
const INSTANCE_IMG_SIZE_BYTES: i64 = 1 << 20; // 1MB
const WRITE_BUFFER_CAPACITY: usize = 512;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

static PENDING_REQUESTS: AtomicCounter = AtomicCounter::new();
static SERVICE_VM: Mutex<Option<ServiceVm>> = Mutex::new(None);
static SERVICE_VM_SHUTDOWN: Condvar = Condvar::new();

/// Atomic counter with a condition variable that is used to wait for the counter
/// to become positive within a timeout.
#[derive(Debug, Default)]
struct AtomicCounter {
    num: Mutex<usize>,
    num_increased: Condvar,
}

impl AtomicCounter {
    const fn new() -> Self {
        Self { num: Mutex::new(0), num_increased: Condvar::new() }
    }

    /// Checks if the counter becomes positive within the given timeout.
    fn is_positive_within_timeout(&self, timeout: Duration) -> bool {
        let (guard, _wait_result) = self
            .num_increased
            .wait_timeout_while(self.num.lock().unwrap(), timeout, |&mut x| x == 0)
            .unwrap();
        *guard > 0
    }

    fn increment(&self) {
        let mut num = self.num.lock().unwrap();
        *num = num.checked_add(1).unwrap();
        self.num_increased.notify_all();
    }

    fn decrement(&self) {
        let mut num = self.num.lock().unwrap();
        *num = num.checked_sub(1).unwrap();
    }
}

/// Processes the request in the service VM.
pub fn process_request(request: Request) -> Result<Response> {
    PENDING_REQUESTS.increment();
    let result = process_request_in_service_vm(request);
    PENDING_REQUESTS.decrement();
    thread::spawn(stop_service_vm_if_idle);
    result
}

fn process_request_in_service_vm(request: Request) -> Result<Response> {
    let mut service_vm = SERVICE_VM.lock().unwrap();
    if service_vm.is_none() {
        *service_vm = Some(ServiceVm::start()?);
    }
    service_vm.as_mut().unwrap().process_request(request)
}

fn stop_service_vm_if_idle() {
    if PENDING_REQUESTS.is_positive_within_timeout(Duration::from_secs(1)) {
        info!("Service VM has pending requests, keeping it running.");
    } else {
        info!("Service VM is idle, shutting it down.");
        *SERVICE_VM.lock().unwrap() = None;
        SERVICE_VM_SHUTDOWN.notify_all();
    }
}

/// Waits until the service VM shuts down.
/// This function is only used in tests.
pub fn wait_until_service_vm_shuts_down() -> Result<()> {
    const WAIT_FOR_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

    let (_guard, wait_result) = SERVICE_VM_SHUTDOWN
        .wait_timeout_while(SERVICE_VM.lock().unwrap(), WAIT_FOR_SHUTDOWN_TIMEOUT, |x| x.is_some())
        .unwrap();
    ensure!(!wait_result.timed_out(), "Service VM didn't shut down within the timeout");
    Ok(())
}

/// Service VM.
pub struct ServiceVm {
    vsock_stream: VsockStream,
    /// VmInstance will be dropped when ServiceVm goes out of scope, which will kill the VM.
    vm: VmInstance,
}

impl ServiceVm {
    /// Starts the service VM and returns its instance.
    /// The same instance image is used for different VMs.
    /// TODO(b/27593612): Remove instance image usage for Service VM.
    pub fn start() -> Result<Self> {
        let instance_img_path = Path::new(VIRT_DATA_DIR).join(INSTANCE_IMG_NAME);
        let vm = protected_vm_instance(instance_img_path)?;

        let vm = Self::start_vm(vm, VmType::ProtectedVm)?;
        Ok(vm)
    }

    /// Starts the given VM instance and sets up the vsock connection with it.
    /// Returns a `ServiceVm` instance.
    /// This function is exposed for testing.
    pub fn start_vm(vm: VmInstance, vm_type: VmType) -> Result<Self> {
        // Sets up the vsock server on the host.
        let vsock_listener = VsockListener::bind_with_cid_port(VMADDR_CID_HOST, vm_type.port())?;

        // Starts the service VM.
        vm.start().context("Failed to start service VM")?;
        info!("Service VM started");

        // Accepts the connection from the service VM.
        // TODO(b/299427101): Introduce a timeout for the accept.
        let (vsock_stream, peer_addr) = vsock_listener.accept().context("Failed to accept")?;
        info!("Accepted connection {:?}", vsock_stream);
        ensure!(
            peer_addr.cid() == u32::try_from(vm.cid()).unwrap(),
            "The CID of the peer address {} doesn't match the service VM CID {}",
            peer_addr,
            vm.cid()
        );
        vsock_stream.set_read_timeout(Some(READ_TIMEOUT))?;
        vsock_stream.set_write_timeout(Some(WRITE_TIMEOUT))?;

        Ok(Self { vsock_stream, vm })
    }

    /// Processes the request in the service VM.
    pub fn process_request(&mut self, request: Request) -> Result<Response> {
        self.write_request(&ServiceVmRequest::Process(request))?;
        self.read_response()
    }

    /// Sends the request to the service VM.
    fn write_request(&mut self, request: &ServiceVmRequest) -> Result<()> {
        let mut buffer = BufWriter::with_capacity(WRITE_BUFFER_CAPACITY, &mut self.vsock_stream);
        ciborium::into_writer(request, &mut buffer)?;
        buffer.flush().context("Failed to flush the buffer")?;
        info!("Sent request to the service VM.");
        Ok(())
    }

    /// Reads the response from the service VM.
    fn read_response(&mut self) -> Result<Response> {
        let response: Response = ciborium::from_reader(&mut self.vsock_stream)
            .context("Failed to read the response from the service VM")?;
        info!("Received response from the service VM.");
        Ok(response)
    }

    /// Shuts down the service VM.
    fn shutdown(&mut self) -> Result<DeathReason> {
        self.write_request(&ServiceVmRequest::Shutdown)?;
        self.vm
            .wait_for_death_with_timeout(Duration::from_secs(10))
            .ok_or_else(|| anyhow!("Timed out to exit the service VM"))
    }
}

impl Drop for ServiceVm {
    fn drop(&mut self) {
        // Wait till the service VM finishes releasing all the resources.
        match self.shutdown() {
            Ok(reason) => info!("Exit the service VM successfully: {reason:?}"),
            Err(e) => warn!("Service VM shutdown request failed '{e:?}', killing it."),
        }
    }
}

/// Returns a `VmInstance` of a protected VM with the instance image from the given path.
pub fn protected_vm_instance(instance_img_path: PathBuf) -> Result<VmInstance> {
    let virtmgr = vmclient::VirtualizationService::new().context("Failed to spawn VirtMgr")?;
    let service = virtmgr.connect().context("Failed to connect to VirtMgr")?;
    info!("Connected to VirtMgr for service VM");

    let instance_img = instance_img(service.as_ref(), instance_img_path)?;
    let writable_partitions = vec![Partition {
        label: "vm-instance".to_owned(),
        image: Some(instance_img),
        writable: true,
        guid: None,
    }];
    let rialto = File::open(RIALTO_PATH).context("Failed to open Rialto kernel binary")?;
    let instance_id_file = Path::new(VIRT_DATA_DIR).join(INSTANCE_ID_FILENAME);
    let instance_id = get_or_allocate_instance_id(service.as_ref(), instance_id_file)?;
    let config = VirtualMachineConfig::RawConfig(VirtualMachineRawConfig {
        name: String::from("Service VM"),
        kernel: Some(ParcelFileDescriptor::new(rialto)),
        disks: vec![DiskImage { image: None, partitions: writable_partitions, writable: true }],
        instanceId: instance_id,
        protectedVm: true,
        memoryMib: VM_MEMORY_MB,
        cpuTopology: CpuTopology::ONE_CPU,
        platformVersion: "~1.0".to_string(),
        gdbPort: 0, // No gdb
        ..Default::default()
    });
    let console_out = Some(android_log_fd()?);
    let console_in = None;
    let log = Some(android_log_fd()?);
    let callback = None;
    VmInstance::create(service.as_ref(), &config, console_out, console_in, log, callback)
        .context("Failed to create service VM")
}

/// TODO(b/291213394): Reuse this method in other places such as vm and compos.
fn get_or_allocate_instance_id(
    service: &dyn IVirtualizationService,
    instance_id_file: PathBuf,
) -> Result<[u8; 64]> {
    let mut instance_id = [0; 64];
    if instance_id_file.exists() {
        let mut file = File::open(&instance_id_file)?;
        file.read_exact(&mut instance_id)?;
    } else {
        info!("Allocating a new instance ID for the Service VM");
        instance_id = service.allocateInstanceId()?;
        fs::write(instance_id_file, instance_id)?;
    }
    Ok(instance_id)
}

/// Returns the file descriptor of the instance image at the given path.
fn instance_img(
    service: &dyn IVirtualizationService,
    instance_img_path: PathBuf,
) -> Result<ParcelFileDescriptor> {
    if instance_img_path.exists() {
        // TODO(b/298174584): Try to recover if the service VM is triggered by rkpd.
        return Ok(OpenOptions::new()
            .read(true)
            .write(true)
            .open(instance_img_path)
            .map(ParcelFileDescriptor::new)?);
    }
    let instance_img = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(instance_img_path)
        .map(ParcelFileDescriptor::new)?;
    service.initializeWritablePartition(
        &instance_img,
        INSTANCE_IMG_SIZE_BYTES,
        PartitionType::ANDROID_VM_INSTANCE,
    )?;
    Ok(instance_img)
}

/// This function is only exposed for testing.
pub fn android_log_fd() -> io::Result<File> {
    let (reader_fd, writer_fd) = nix::unistd::pipe()?;

    let reader = File::from(reader_fd);
    let writer = File::from(writer_fd);

    thread::spawn(|| {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(l) => info!("{}", l),
                Err(e) => {
                    warn!("Failed to read line: {e:?}");
                    break;
                }
            }
        }
    });
    Ok(writer)
}
