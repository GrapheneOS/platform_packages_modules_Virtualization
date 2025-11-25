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
use libloading::Library;
use log::{info, LevelFilter};
use std::ffi::{c_void, CStr};
use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os::fd::IntoRawFd;
use std::os::raw::c_int;
use std::sync::mpsc::{self, Sender};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use vsock::{VsockListener, VsockStream, VMADDR_CID_HOST};

use avf_bindgen::*;
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

/// Processes the request in the service VM.
fn process_request(vsock_stream: &mut VsockStream, request: Request) -> Result<Response> {
    write_request(vsock_stream, &request)?;
    read_response(vsock_stream)
}

/// Sends the request to the service VM.
fn write_request(vsock_stream: &mut VsockStream, request: &Request) -> Result<()> {
    let mut buffer = BufWriter::with_capacity(WRITE_BUFFER_CAPACITY, vsock_stream);
    ciborium::into_writer(request, &mut buffer)?;
    buffer.flush().context("Failed to flush the buffer")?;
    Ok(())
}

/// Reads the response from the service VM.
fn read_response(vsock_stream: &mut VsockStream) -> Result<Response> {
    let response: Response = ciborium::from_reader(vsock_stream)
        .context("Failed to read the response from the service VM")?;
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
    TestFn: FnOnce(&mut VsockStream) -> Result<()>,
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
    let mut supports_callback: bool = false;

    // SAFETY:
    //   - vm is the only reference to a valid object
    //   - libavf.so is guaranteed by precondition check in AndroidTest.xml.
    //   - AVirtualMachine_startWithStopCallback is released as LLNDK, hence interface is stable.
    unsafe {
        let lib = Library::new("libavf.so").unwrap();
        let start_with_callback: Result<
            libloading::Symbol<
                unsafe extern "C" fn(
                    *mut AVirtualMachine,
                    AVirtualMachine_stopCallback,
                    *mut c_void,
                ) -> c_int,
            >,
            _,
        > = lib.get(b"AVirtualMachine_startWithStopCallback");

        // With trunk stable, this test may run on device without the start_with_callback.
        // Only test with callbacks when the API is available.
        // TODO: Remove this block when the API is fully deployed, or invent better way
        //       to do this.
        if let Ok(start_with_callback) = start_with_callback {
            info!("starting VM with AVirtualMachine_startWithStopCallback");
            supports_callback = true;
            start_with_callback(vm, Some(on_stopped), &mut callback_data as *mut _ as *mut c_void);
        } else {
            info!("starting VM with AVirtualMachine_start");
            AVirtualMachine_start(vm);
        }
    }

    let vm_ptr = vm as usize;

    info!("VM started");

    let mut vsock_stream = listener_thread.join().unwrap()?;
    vsock_stream.set_read_timeout(Some(READ_TIMEOUT))?;
    vsock_stream.set_write_timeout(Some(WRITE_TIMEOUT))?;

    info!("client connected");

    test_fn(&mut vsock_stream)?;

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

fn run_reverse_test(vsock_stream: &mut VsockStream) -> Result<()> {
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
fn test_run_service_vm_protected() -> Result<()> {
    init_logger();

    run_test(c"vts_libavf_test_service_vm", VmType::Protected, run_reverse_test)
}

#[test]
fn test_run_service_vm_non_protected() -> Result<()> {
    init_logger();

    run_test(c"vts_libavf_test_service_vm", VmType::NonProtected, run_reverse_test)
}
