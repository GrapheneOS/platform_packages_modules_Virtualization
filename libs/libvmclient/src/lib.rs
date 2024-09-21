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

//! Client library for VirtualizationService.

mod death_reason;
mod error_code;
mod errors;
mod sync;

pub use crate::death_reason::DeathReason;
pub use crate::error_code::ErrorCode;
pub use crate::errors::VmWaitError;
use crate::sync::Monitor;
use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::{
    DeathReason::DeathReason as AidlDeathReason, ErrorCode::ErrorCode as AidlErrorCode,
};
use android_system_virtualizationservice::{
    aidl::android::system::virtualizationservice::{
        IVirtualMachine::IVirtualMachine,
        IVirtualMachineCallback::{BnVirtualMachineCallback, IVirtualMachineCallback},
        IVirtualizationService::IVirtualizationService,
        VirtualMachineConfig::VirtualMachineConfig,
        VirtualMachineState::VirtualMachineState,
    },
    binder::{
        BinderFeatures, DeathRecipient, FromIBinder, IBinder, Interface, ParcelFileDescriptor,
        Result as BinderResult, StatusCode, Strong,
    },
};
use command_fds::CommandFdExt;
use log::warn;
use rpcbinder::{FileDescriptorTransportMode, RpcSession};
use shared_child::SharedChild;
use std::ffi::{c_char, c_int, c_void, CString};
use std::io::{self, Read};
use std::os::fd::RawFd;
use std::process::Command;
use std::{
    fmt::{self, Debug, Formatter},
    fs::File,
    os::unix::io::{AsFd, AsRawFd, IntoRawFd, OwnedFd},
    sync::Arc,
    time::Duration,
};

const EARLY_VIRTMGR_PATH: &str = "/apex/com.android.virt/bin/early_virtmgr";
const VIRTMGR_PATH: &str = "/apex/com.android.virt/bin/virtmgr";
const VIRTMGR_THREADS: usize = 2;

fn posix_pipe() -> Result<(OwnedFd, OwnedFd), io::Error> {
    use nix::fcntl::OFlag;
    use nix::unistd::pipe2;

    // Create new POSIX pipe. Make it O_CLOEXEC to align with how Rust creates
    // file descriptors (expected by SharedChild).
    Ok(pipe2(OFlag::O_CLOEXEC)?)
}

fn posix_socketpair() -> Result<(OwnedFd, OwnedFd), io::Error> {
    use nix::sys::socket::{socketpair, AddressFamily, SockFlag, SockType};

    // Create new POSIX socketpair, suitable for use with RpcBinder UDS bootstrap
    // transport. Make it O_CLOEXEC to align with how Rust creates file
    // descriptors (expected by SharedChild).
    Ok(socketpair(AddressFamily::Unix, SockType::Stream, None, SockFlag::SOCK_CLOEXEC)?)
}

/// Error handling function for `get_virtualization_service`.
///
/// # Safety
/// `message` shouldn't be used outside of the lifetime of the function. Management of `ctx` is
/// entirely up to the function.
pub type ErrorCallback =
    unsafe extern "C" fn(code: c_int, message: *const c_char, ctx: *mut c_void);

/// Spawns a new instance of virtmgr and rerturns a file descriptor for the socket connection to
/// the service. When error occurs, it is reported via the ErrorCallback function along with the
/// error message and any context that is set by the client.
///
/// # Safety
/// `cb` should be null or a valid function pointer of type `ErrorCallback`
#[no_mangle]
pub unsafe extern "C" fn get_virtualization_service(
    cb: Option<ErrorCallback>,
    ctx: *mut c_void,
) -> RawFd {
    match VirtualizationService::new() {
        Ok(vs) => vs.client_fd.into_raw_fd(),
        Err(e) => {
            if let Some(cb) = cb {
                let code = e.raw_os_error().unwrap_or(-1);
                let msg = CString::new(e.to_string()).unwrap();
                // SAFETY: `cb` doesn't use `msg` outside of the lifetime of the function.
                // msg's lifetime is longer than `cb` as it is bound to a local variable.
                unsafe { cb(code, msg.as_ptr(), ctx) };
            }
            -1
        }
    }
}

/// A running instance of virtmgr which is hosting a VirtualizationService
/// RpcBinder server.
pub struct VirtualizationService {
    /// Client FD for UDS connection to virtmgr's RpcBinder server. Closing it
    /// will make virtmgr shut down.
    client_fd: OwnedFd,
}

impl VirtualizationService {
    /// Spawns a new instance of virtmgr, a child process that will host
    /// the VirtualizationService AIDL service.
    pub fn new() -> Result<VirtualizationService, io::Error> {
        Self::new_with_path(VIRTMGR_PATH)
    }

    /// Spawns a new instance of early_virtmgr, a child process that will host
    /// the VirtualizationService AIDL service for early VMs.
    pub fn new_early() -> Result<VirtualizationService, io::Error> {
        Self::new_with_path(EARLY_VIRTMGR_PATH)
    }

    fn new_with_path(virtmgr_path: &str) -> Result<VirtualizationService, io::Error> {
        let (wait_fd, ready_fd) = posix_pipe()?;
        let (client_fd, server_fd) = posix_socketpair()?;

        let mut command = Command::new(virtmgr_path);
        // Can't use BorrowedFd as it doesn't implement Display
        command.arg("--rpc-server-fd").arg(format!("{}", server_fd.as_raw_fd()));
        command.arg("--ready-fd").arg(format!("{}", ready_fd.as_raw_fd()));
        command.preserved_fds(vec![server_fd, ready_fd]);

        SharedChild::spawn(&mut command)?;

        // Wait for the child to signal that the RpcBinder server is read by closing its end of the
        // pipe. Failing to read (especially EACCESS or EPERM) can happen if the client lacks the
        // MANAGE_VIRTUAL_MACHINE permission. Therefore, such errors are propagated instead of
        // being ignored.
        let _ = File::from(wait_fd).read(&mut [0])?;
        Ok(VirtualizationService { client_fd })
    }

    /// Connects to the VirtualizationService AIDL service.
    pub fn connect(&self) -> Result<Strong<dyn IVirtualizationService>, io::Error> {
        let session = RpcSession::new();
        session.set_file_descriptor_transport_mode(FileDescriptorTransportMode::Unix);
        session.set_max_incoming_threads(VIRTMGR_THREADS);
        session
            .setup_unix_domain_bootstrap_client(self.client_fd.as_fd())
            .map_err(|_| io::Error::from(io::ErrorKind::ConnectionRefused))
    }
}

/// A virtual machine which has been started by the VirtualizationService.
pub struct VmInstance {
    /// The `IVirtualMachine` Binder object representing the VM.
    pub vm: Strong<dyn IVirtualMachine>,
    cid: i32,
    state: Arc<Monitor<VmState>>,
    // Ensure that the DeathRecipient isn't dropped while someone might call wait_for_death, as it
    // is removed from the Binder when it's dropped.
    _death_recipient: DeathRecipient,
}

/// A trait to be implemented by clients to handle notification of significant changes to the VM
/// state. Default implementations of all functions are provided so clients only need to handle the
/// notifications they are interested in.
#[allow(unused_variables)]
pub trait VmCallback {
    /// Called when the payload has been started within the VM. If present, `stream` is connected
    /// to the stdin/stdout of the payload.
    fn on_payload_started(&self, cid: i32) {}

    /// Callend when the payload has notified Virtualization Service that it is ready to serve
    /// clients.
    fn on_payload_ready(&self, cid: i32) {}

    /// Called when the payload has exited in the VM. `exit_code` is the exit code of the payload
    /// process.
    fn on_payload_finished(&self, cid: i32, exit_code: i32) {}

    /// Called when an error has occurred in the VM. The `error_code` and `message` may give
    /// further details.
    fn on_error(&self, cid: i32, error_code: ErrorCode, message: &str) {}

    /// Called when the VM has exited, all resources have been freed, and any logs have been
    /// written. `death_reason` gives an indication why the VM exited.
    fn on_died(&self, cid: i32, death_reason: DeathReason) {}
}

impl VmInstance {
    /// Creates (but doesn't start) a new VM with the given configuration.
    pub fn create(
        service: &dyn IVirtualizationService,
        config: &VirtualMachineConfig,
        console_out: Option<File>,
        console_in: Option<File>,
        log: Option<File>,
        callback: Option<Box<dyn VmCallback + Send + Sync>>,
    ) -> BinderResult<Self> {
        let console_out = console_out.map(ParcelFileDescriptor::new);
        let console_in = console_in.map(ParcelFileDescriptor::new);
        let log = log.map(ParcelFileDescriptor::new);

        let vm =
            service.createVm(config, console_out.as_ref(), console_in.as_ref(), log.as_ref())?;

        let cid = vm.getCid()?;

        // Register callback before starting VM, in case it dies immediately.
        let state = Arc::new(Monitor::new(VmState::default()));
        let callback = BnVirtualMachineCallback::new_binder(
            VirtualMachineCallback { state: state.clone(), client_callback: callback },
            BinderFeatures::default(),
        );
        vm.registerCallback(&callback)?;
        let death_recipient = wait_for_binder_death(&mut vm.as_binder(), state.clone())?;

        Ok(Self { vm, cid, state, _death_recipient: death_recipient })
    }

    /// Starts the VM.
    pub fn start(&self) -> BinderResult<()> {
        self.vm.start()
    }

    /// Returns the CID used for vsock connections to the VM.
    pub fn cid(&self) -> i32 {
        self.cid
    }

    /// Returns the current lifecycle state of the VM.
    pub fn state(&self) -> BinderResult<VirtualMachineState> {
        self.vm.getState()
    }

    /// Blocks until the VM or the VirtualizationService itself dies, and then returns the reason
    /// why it died.
    pub fn wait_for_death(&self) -> DeathReason {
        self.state.wait_while(|state| state.death_reason.is_none()).unwrap().death_reason.unwrap()
    }

    /// Blocks until the VM or the VirtualizationService itself dies, or the given timeout expires.
    /// Returns the reason why it died if it did so.
    pub fn wait_for_death_with_timeout(&self, timeout: Duration) -> Option<DeathReason> {
        let (state, _timeout_result) =
            self.state.wait_timeout_while(timeout, |state| state.death_reason.is_none()).unwrap();
        // We don't care if it timed out - we just return the reason if there now is one
        state.death_reason
    }

    /// Waits until the VM reports that it is ready.
    ///
    /// Returns an error if the VM dies first, or the `timeout` elapses before the VM is ready.
    pub fn wait_until_ready(&self, timeout: Duration) -> Result<(), VmWaitError> {
        let (state, timeout_result) = self
            .state
            .wait_timeout_while(timeout, |state| {
                state.reported_state < VirtualMachineState::READY && state.death_reason.is_none()
            })
            .unwrap();
        if timeout_result.timed_out() {
            Err(VmWaitError::TimedOut)
        } else if let Some(reason) = state.death_reason {
            Err(VmWaitError::Died { reason })
        } else if state.reported_state != VirtualMachineState::READY {
            Err(VmWaitError::Finished)
        } else {
            Ok(())
        }
    }

    /// Tries to connect to an RPC Binder service provided by the VM on the given vsock port.
    pub fn connect_service<T: FromIBinder + ?Sized>(
        &self,
        port: u32,
    ) -> Result<Strong<T>, StatusCode> {
        RpcSession::new().setup_preconnected_client(|| {
            match self.vm.connectVsock(port as i32) {
                Ok(vsock) => {
                    // Ownership of the fd is transferred to binder
                    Some(vsock.into_raw_fd())
                }
                Err(e) => {
                    warn!("Vsock connection failed: {}", e);
                    None
                }
            }
        })
    }
}

impl Debug for VmInstance {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.debug_struct("VmInstance").field("cid", &self.cid).field("state", &self.state).finish()
    }
}

/// Notify the VmState when the given Binder object dies.
///
/// If the returned DeathRecipient is dropped then this will no longer do anything.
fn wait_for_binder_death(
    binder: &mut impl IBinder,
    state: Arc<Monitor<VmState>>,
) -> BinderResult<DeathRecipient> {
    let mut death_recipient = DeathRecipient::new(move || {
        warn!("VirtualizationService unexpectedly died");
        state.notify_death(DeathReason::VirtualizationServiceDied);
    });
    binder.link_to_death(&mut death_recipient)?;
    Ok(death_recipient)
}

#[derive(Debug, Default)]
struct VmState {
    death_reason: Option<DeathReason>,
    reported_state: VirtualMachineState,
}

impl Monitor<VmState> {
    fn notify_death(&self, reason: DeathReason) {
        let state = &mut *self.state.lock().unwrap();
        // In case this method is called more than once, ignore subsequent calls.
        if state.death_reason.is_none() {
            state.death_reason.replace(reason);
            self.cv.notify_all();
        }
    }

    fn notify_state(&self, state: VirtualMachineState) {
        self.state.lock().unwrap().reported_state = state;
        self.cv.notify_all();
    }
}

struct VirtualMachineCallback {
    state: Arc<Monitor<VmState>>,
    client_callback: Option<Box<dyn VmCallback + Send + Sync>>,
}

impl Debug for VirtualMachineCallback {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("VirtualMachineCallback")
            .field("state", &self.state)
            .field(
                "client_callback",
                &if self.client_callback.is_some() { "Some(...)" } else { "None" },
            )
            .finish()
    }
}

impl Interface for VirtualMachineCallback {}

impl IVirtualMachineCallback for VirtualMachineCallback {
    fn onPayloadStarted(&self, cid: i32) -> BinderResult<()> {
        self.state.notify_state(VirtualMachineState::STARTED);
        if let Some(ref callback) = self.client_callback {
            callback.on_payload_started(cid);
        }
        Ok(())
    }

    fn onPayloadReady(&self, cid: i32) -> BinderResult<()> {
        self.state.notify_state(VirtualMachineState::READY);
        if let Some(ref callback) = self.client_callback {
            callback.on_payload_ready(cid);
        }
        Ok(())
    }

    fn onPayloadFinished(&self, cid: i32, exit_code: i32) -> BinderResult<()> {
        self.state.notify_state(VirtualMachineState::FINISHED);
        if let Some(ref callback) = self.client_callback {
            callback.on_payload_finished(cid, exit_code);
        }
        Ok(())
    }

    fn onError(&self, cid: i32, error_code: AidlErrorCode, message: &str) -> BinderResult<()> {
        self.state.notify_state(VirtualMachineState::FINISHED);
        if let Some(ref callback) = self.client_callback {
            let error_code = error_code.into();
            callback.on_error(cid, error_code, message);
        }
        Ok(())
    }

    fn onDied(&self, cid: i32, reason: AidlDeathReason) -> BinderResult<()> {
        let reason = reason.into();
        self.state.notify_death(reason);
        if let Some(ref callback) = self.client_callback {
            callback.on_died(cid, reason);
        }
        Ok(())
    }
}
