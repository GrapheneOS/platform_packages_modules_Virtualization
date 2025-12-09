// Copyright 2024 The Android Open Source Project
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

//! Tests running a VM with LLNDK

use android_logger::Config;
use anyhow::{bail, ensure, Context, Result};
use log::{info, LevelFilter};
use nix::fcntl::OFlag;
use std::ffi::{c_void, CStr};
use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::sync::mpsc::{self, Sender};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use vsock::{VsockListener, VsockStream, VMADDR_CID_HOST};

use avf_compat_bindgen::*;
use dma_buf_heap_bindgen::{dma_heap_allocation_data, DMA_HEAP_IOC_MAGIC};
use memmap2::MmapOptions;
use vmbase_test_vm_messages::{Request, Response, VM_PORT};

const LOG_TAG: &str = "VtsLibAvf";

const VM_MEMORY_MB: i32 = 16;
const WRITE_BUFFER_CAPACITY: usize = 512;

const LISTEN_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: timespec = timespec { tv_sec: 10, tv_nsec: 0 };

static ON_STOPPED_EVENT: LazyLock<Mutex<Sender<(usize, AVirtualMachineStopReason, u8)>>> =
    LazyLock::new(|| {
        // Returning stub here because `Receiver` isn't `Sync`.
        let (tx, _) = mpsc::channel();
        Mutex::new(tx)
    });

/// Processes the request in the test VM.
fn process_request(vsock_stream: &mut VsockStream, request: Request) -> Result<Response> {
    write_request(vsock_stream, &request)?;
    read_response(vsock_stream)
}

/// Sends the request to the test VM.
fn write_request(vsock_stream: &mut VsockStream, request: &Request) -> Result<()> {
    let mut buffer = BufWriter::with_capacity(WRITE_BUFFER_CAPACITY, vsock_stream);
    ciborium::into_writer(request, &mut buffer)?;
    buffer.flush().context("Failed to flush the buffer")?;
    Ok(())
}

/// Reads the response from the test VM.
fn read_response(vsock_stream: &mut VsockStream) -> Result<Response> {
    let response: Response = ciborium::from_reader(vsock_stream)
        .context("Failed to read the response from the test VM")?;
    Ok(response)
}

fn listen_from_guest(port: u32) -> Result<VsockStream> {
    let vsock_listener =
        VsockListener::bind_with_cid_port(VMADDR_CID_HOST, port).context("Failed to bind vsock")?;
    vsock_listener.set_nonblocking(true).context("Failed to set nonblocking")?;
    let start_time = Instant::now();
    loop {
        if start_time.elapsed() >= LISTEN_TIMEOUT {
            bail!("Timeout while listening");
        }
        match vsock_listener.accept() {
            Ok((vsock_stream, _peer_addr)) => return Ok(vsock_stream),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => bail!("Failed to listen: {e:?}"),
        }
    }
}

unsafe extern "C" fn on_stopped(
    vm: *mut AVirtualMachine,
    reason: AVirtualMachineStopReason,
    data: *mut c_void,
) {
    info!("on_stopped");

    // SAFETY: `data` is a valid pointer passed by AVirtualMachine_start.
    let data = unsafe { *(data as *const u8) };
    ON_STOPPED_EVENT.lock().unwrap().send((vm as usize, reason, data)).unwrap();
}

#[derive(Debug)]
enum VmType {
    Protected,
    NonProtected,
}

impl VmType {
    fn is_supported(&self) -> Result<bool> {
        match self {
            VmType::Protected => hypervisor_props::is_protected_vm_supported(),
            VmType::NonProtected => hypervisor_props::is_vm_supported(),
        }
    }
}

impl fmt::Display for VmType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            VmType::Protected => write!(f, "protected"),
            VmType::NonProtected => write!(f, "non-protected"),
        }
    }
}

fn run_test<TestFn>(test_vm_name: &CStr, vm_type: VmType, test_fn: TestFn) -> Result<()>
where
    TestFn: FnOnce(&mut VsockStream, *mut AVirtualMachine) -> Result<()>,
{
    if !vm_type.is_supported()? {
        info!("{vm_type} VMs are not supported. skipping test");
        return Ok(());
    }

    let kernel_file = File::open("/data/nativetest64/vendor/vts_libavf_vm.bin")
        .context("Failed to open kernel file")?;
    let kernel_fd = kernel_file.into_raw_fd();

    let (tx, rx) = mpsc::channel();
    (*ON_STOPPED_EVENT.lock().unwrap()) = tx;

    // SAFETY: AVirtualMachineRawConfig_create() isn't unsafe but rust_bindgen forces it to be seen
    // as unsafe
    let config = unsafe { AVirtualMachineRawConfig_create() };

    info!("raw config created");

    // The first 4 bytes of an instance ID for a vendor VM must be 0xFFFFFFFF
    let mut instance_id: [u8; 64] = [0; 64];
    instance_id[0..4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);

    // SAFETY: config is the only reference to a valid object
    unsafe {
        AVirtualMachineRawConfig_setName(config, test_vm_name.as_ptr());
        AVirtualMachineRawConfig_setKernel(config, kernel_fd);
        AVirtualMachineRawConfig_setProtectedVm(config, matches!(vm_type, VmType::Protected));
        AVirtualMachineRawConfig_setMemoryMiB(config, VM_MEMORY_MB);
        AVirtualMachineRawConfig_setInstanceId(config, instance_id.as_ptr(), instance_id.len());
    }

    let mut vm = std::ptr::null_mut();
    let mut service = std::ptr::null_mut();

    ensure!(
        // SAFETY: &mut service is a valid pointer to *AVirtualizationService
        unsafe { AVirtualizationService_create(&mut service, false) } == 0,
        "AVirtualizationService_create failed"
    );

    scopeguard::defer! {
        // SAFETY: service is a valid pointer to AVirtualizationService
        unsafe { AVirtualizationService_destroy(service); }
    }

    ensure!(
        // SAFETY: &mut vm is a valid pointer to *AVirtualMachine
        unsafe {
            AVirtualMachine_createRaw(
                service, config, -1, // console_in
                -1, // console_out
                -1, // log
                &mut vm,
            )
        } == 0,
        "AVirtualMachine_createRaw failed"
    );

    // Note: You can call AVirtualMachine_destroy() inside the stop callback.
    scopeguard::defer! {
        // SAFETY: vm is a valid pointer to AVirtualMachine
        unsafe { AVirtualMachine_destroy(vm); }
    }

    info!("vm created");

    let listener_thread = std::thread::spawn(move || listen_from_guest(VM_PORT));

    let mut callback_data = 33_u8;
    let mut supports_callback = false;

    // SAFETY: vm is the only reference to a valid object, on_stopped is a correct callback and
    // callback data is a valid pointer.
    unsafe {
        AVirtualMachineCompat_startWithStopCallback(
            vm,
            Some(on_stopped),
            &mut callback_data as *mut _ as *mut c_void,
            &mut supports_callback,
        )
    };

    let vm_ptr = vm as usize;

    info!("VM started");

    let mut vsock_stream = listener_thread.join().unwrap()?;
    vsock_stream.set_read_timeout(Some(READ_TIMEOUT))?;
    vsock_stream.set_write_timeout(Some(WRITE_TIMEOUT))?;

    info!("client connected");

    test_fn(&mut vsock_stream, vm)?;

    write_request(&mut vsock_stream, &Request::Shutdown).context("Failed to send shutdown")?;

    info!("shutdown sent");

    let mut stop_reason = AVirtualMachineStopReason::AVIRTUAL_MACHINE_UNRECOGNISED;
    ensure!(
        // SAFETY: vm is the only reference to a valid object
        unsafe { AVirtualMachine_waitForStop(vm, &STOP_TIMEOUT, &mut stop_reason) },
        "AVirtualMachine_waitForStop failed"
    );

    assert_eq!(AVirtualMachineStopReason::AVIRTUAL_MACHINE_SHUTDOWN, stop_reason);

    info!("stopped");

    if supports_callback {
        info!("checking that callback was invoked");
        let timeout = Duration::from_secs(STOP_TIMEOUT.tv_sec.try_into().unwrap());
        let (stopped_vm_ptr, stopped_reason, stopped_callback_data) =
            rx.recv_timeout(timeout).expect("Callback should have been called");
        assert_eq!(stopped_vm_ptr, vm_ptr);
        assert_eq!(stopped_reason, stop_reason);
        assert_eq!(stopped_callback_data, callback_data);
    }

    Ok(())
}

fn init_logger() {
    android_logger::init_once(
        Config::default()
            .with_tag(LOG_TAG)
            .with_max_level(LevelFilter::Info)
            .with_log_buffer(android_logger::LogId::System),
    );
}

fn run_reverse_test(vsock_stream: &mut VsockStream, _: *mut AVirtualMachine) -> Result<()> {
    let request_data = vec![1, 2, 3, 4, 5];
    let expected_data = vec![5, 4, 3, 2, 1];
    let response = process_request(vsock_stream, Request::Reverse(request_data))
        .context("Failed to process request")?;
    let Response::Reverse(reversed_data) = response else {
        bail!("Expected `Response::Reverse` but was {response:?}");
    };
    ensure!(reversed_data == expected_data, "Expected {expected_data:?} but was {reversed_data:?}");
    info!("request processed");
    Ok(())
}

#[test]
fn test_run_test_vm_protected() -> Result<()> {
    init_logger();

    run_test(c"vts_libavf_test_vm", VmType::Protected, run_reverse_test)
}

#[test]
fn test_run_test_vm_non_protected() -> Result<()> {
    init_logger();

    run_test(c"vts_libavf_test_vm", VmType::NonProtected, run_reverse_test)
}

#[test]
fn test_share_dma_buf_4096_protected_vm() -> Result<()> {
    init_logger();

    run_test(c"vts_libavf_test_share_dma_buf_4096", VmType::Protected, |vsock_stream, vm| {
        share_dma_buf_test_impl(vsock_stream, vm, 4096, 4096)
    })
}

#[test]
fn test_share_dma_buf_16384_protected_vm() -> Result<()> {
    init_logger();

    run_test(c"vts_libavf_test_share_dma_buf_16384", VmType::Protected, |vsock_stream, vm| {
        share_dma_buf_test_impl(vsock_stream, vm, 16384, 16384)
    })
}

#[test]
fn test_share_dma_buf_32768_protected_vm() -> Result<()> {
    init_logger();

    run_test(c"vts_libavf_test_share_dma_buf_32768", VmType::Protected, |vsock_stream, vm| {
        share_dma_buf_test_impl(vsock_stream, vm, 32768, 32768)
    })
}

#[test]
fn test_share_dma_buf_2097152_protected_vm() -> Result<()> {
    init_logger();

    run_test(c"vts_libavf_test_share_dma_buf_2097152", VmType::Protected, |vsock_stream, vm| {
        // Sending 2 MiBs of data in a single chunk over vsock results in a VM crash because
        // vmbase_test_vm has swiotlb backed by heap. A 16 KiB is a good as any other size of the
        // chunk ¯\_(ツ)_/¯
        share_dma_buf_test_impl(vsock_stream, vm, 2097152, 16384)
    })
}

const TEST_DATA_PATTERN: u64 = 0x0021_6B4F_206C_6C41;

fn share_dma_buf_test_impl(
    vsock_stream: &mut VsockStream,
    vm: *mut AVirtualMachine,
    size: usize,
    chunk_size: usize,
) -> Result<()> {
    if !hypervisor_props::is_dynamic_zero_copy_memshare_supported()? {
        info!("dynamic zero copy memory share is not supported by hypervisor. skipping test");
        return Ok(());
    }

    let dma_buf_fd = dma_buf_alloc(size).context("failed to allocate dma_buf_fd")?;

    // Fill the buffer with the test data it deserves.
    let data = TEST_DATA_PATTERN.to_le_bytes().to_vec().repeat(size / 8);

    // SAFETY: dma_buf_fd is valid fd that can be memory mapped.
    let mut mmap = unsafe { MmapOptions::new().len(size).offset(0).map_mut(&dma_buf_fd) }
        .context("failed to mmap")?;
    mmap.copy_from_slice(&data);

    // Pick a range that is outside the main memory region of the VM.
    let range_start = 0x8000_0000 + 32 * 1024 * 1024;
    let range_end = range_start + size;
    // SAFETY: vm is valid pointer to AVirtualMachine. dma_buf_fd is a valid fd which ownership
    // gets transferred to the `AVirtualMachine_addMemoryMapping`.
    let memory_id = unsafe {
        AVirtualMachineCompat_addMemoryMapping(
            vm,
            dma_buf_fd.into_raw_fd(),
            range_start as u64,
            range_end as u64,
            0 /* offset */,
            AVirtualMachineMemoryMappingAttributes::AVIRTUAL_MACHINE_MEMORY_MAPPING_ATTRIBUTE_CACHE_COHERENT,
        )
    };
    ensure!(memory_id >= 0, "AVirtualMachine_addMemoryMapping failed");

    let response = process_request(vsock_stream, Request::MapData(range_start, range_end))
        .context("failed to process request")?;

    let Response::MapData(success) = response else {
        bail!("Expected Response::MapData but was {response:?}");
    };
    ensure!(success, "Request::MapData failed");

    let mut cur_range_start = range_start;
    let mut cur_chunk_idx = 0;
    while cur_range_start < range_end {
        let response = process_request(
            vsock_stream,
            Request::ReadMappedData(cur_range_start, cur_range_start + chunk_size),
        )
        .context("failed to process request")?;
        let Response::ReadMappedData(mapped_data) = response else {
            bail!("Expected Response::ReadMappedData but was {response:?}");
        };

        assert_eq!(mapped_data, data[cur_chunk_idx..(cur_chunk_idx + chunk_size)]);
        cur_range_start += chunk_size;
        cur_chunk_idx += chunk_size;
    }

    let response = process_request(vsock_stream, Request::MemRelinquish(range_start, range_end))
        .context("failed to process request")?;
    let Response::MemRelinquish(success) = response else {
        bail!("Expected Response::MemRelinquish but was {response:?}");
    };
    ensure!(success, "Request::MemRelinquish failed");

    // SAFETY: vm is a valid pointer to AVirtualMachine.
    let success = unsafe { AVirtualMachineCompat_removeMemoryMapping(vm, memory_id) };
    ensure!(success, "AVirtualMachine_removeMemoryMapping failed");

    Ok(())
}

// TODO(ioffe): move code below in libdma_buf_heap library?
mod ioctl {
    use super::*;

    nix::ioctl_readwrite!(dma_heap_alloc, DMA_HEAP_IOC_MAGIC, 0, dma_heap_allocation_data);
}

const DMA_HEAP_SYSTEM: &str = "/dev/dma_heap/system";

fn dma_buf_alloc(size: usize) -> Result<OwnedFd> {
    let dma_heap = File::open(DMA_HEAP_SYSTEM).context("failed to open {DMA_HEAP_SYSTEM}")?;

    let mut data = dma_heap_allocation_data {
        len: size as u64,
        fd: 0,
        fd_flags: OFlag::O_RDWR.bits() as u32 | OFlag::O_CLOEXEC.bits() as u32,
        heap_flags: 0,
    };

    // SAFETY: dma_heap is opened /dev/dma_heap/system and data is valid dma_heap_allocation_data
    match unsafe { ioctl::dma_heap_alloc(dma_heap.as_raw_fd(), &mut data) } {
        Ok(_) => {
            // SAFETY: `data.fd` is valid dma_buf_fd created by kernel.
            unsafe { Ok(OwnedFd::from_raw_fd(data.fd as RawFd)) }
        }
        Err(_) => bail!("dma_heap_alloc failed: {:#?}", io::Error::last_os_error()),
    }
}
