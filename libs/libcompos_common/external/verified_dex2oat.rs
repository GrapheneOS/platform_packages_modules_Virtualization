// Copyright (C) 2025 The Android Open Source Project
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! This crate provides a C-compatible Foreign Function Interface (FFI) for performing
//! verified `dex2oat` compilations within a protected virtual machine.
//!
//! It allows C/C++ clients to create a compilation context, start a compilation with
//! specific arguments, and cancel it if needed. The results of the compilation are
//! reported asynchronously via the caller provided C-style callbacks.

mod wrappers;

#[cfg(not(test))]
use crate::wrappers::binder::wait_for_composd_interface;
#[cfg(test)]
use crate::wrappers::mock_binder::wait_for_composd_interface;

use android_system_composd::aidl::android::system::composd::{
    ICompilationTask::ICompilationTask,
    IDex2OatTaskCallback::{
        BnDex2OatTaskCallback, Dex2OatMetrics::Dex2OatMetrics,
        FailureReason::FailureReason as Dex2OatFailureReason, IDex2OatTaskCallback,
    },
    IIsolatedCompilationService::{
        Dex2OatArg::Dex2OatArg as BnDex2OatArg, IIsolatedCompilationService,
    },
};
use anyhow::Error;
use binder::{ParcelFileDescriptor, Strong};
use compos_bindgen::{
    AVerifiedDex2Oat_CompilationContext as FFICompilationContext,
    AVerifiedDex2Oat_FailureCallbackContext as FFIFailureCallbackContext,
    AVerifiedDex2Oat_FailureReason as FFIFailureReason,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_COMPILATION_SETUP_FAILED as FFIFailureReason_COMPILATION_SETUP_FAILED,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_DEX2OAT_FAILED as FFIFailureReason_DEX2OAT_FAILED,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_FAILED_TO_ENABLE_FSVERITY as FFIFailureReason_FAILED_TO_ENABLE_FSVERITY,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_TIMEOUT as FFIFailureReason_TIMEOUT,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_UNKNOWN as FFIFailureReason_UNKNOWN,
    AVerifiedDex2Oat_FailureResultContext as FFIFailureResultContext,
    AVerifiedDex2Oat_OnFailureCallback as FFIOnFailureCallback,
    AVerifiedDex2Oat_OnSuccessCallback as FFIOnSuccessCallback,
    AVerifiedDex2Oat_Status as FFIStatus,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_BAD_ARGS as FFIStatus_BAD_ARGS,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE as FFIStatus_COMPOS_SERVICE_UNAVAILABLE,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_SUCCESS as FFIStatus_DEX2OAT_SUCCESS,
    AVerifiedDex2Oat_SuccessCallbackContext as FFISuccessCallbackContext,
    AVerifiedDex2Oat_SuccessResultContext as FFISuccessResultContext,
};
use std::{
    ffi::{c_char, c_int, c_void, CString},
    marker::{Send, Sync},
    os::fd::{FromRawFd, OwnedFd},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

/// Represents a single argument for the dex2oat compiler.
///
/// This struct allows passing arguments that may contain file descriptors. The `format_string`
/// can include `!` placeholders, which will be replaced by the file descriptors from the `fds`
/// array.
#[derive(Clone)]
#[repr(C)]
pub struct Dex2OatArg {
    /// A C-style string that can contain `!` placeholders for file descriptors.
    pub format_string: *const c_char,
    /// An array of file descriptors to be substituted into the `format_string`.
    pub fds: *const c_int,
    /// The number of file descriptors in the `fds` array.
    pub num_fds: usize,
}

fn from_dex2oat_failure_reason(reason: Dex2OatFailureReason) -> FFIFailureReason {
    match reason {
        Dex2OatFailureReason::CompilationSetupFailed => FFIFailureReason_COMPILATION_SETUP_FAILED,
        Dex2OatFailureReason::Dex2OatFailed => FFIFailureReason_DEX2OAT_FAILED,
        Dex2OatFailureReason::FailedToEnableFsVerity => FFIFailureReason_FAILED_TO_ENABLE_FSVERITY,
        Dex2OatFailureReason::Timeout => FFIFailureReason_TIMEOUT,
        _ => FFIFailureReason_UNKNOWN,
    }
}

#[derive(Clone)]
struct CallbackContext<T> {
    cb_context: T,
}

// SAFETY: Allowing the void pointer contained within FFISuccessCallbackContext
// to be sent across threads should be safe.
// The void pointer is provided by the caller and it is the caller's responsibility
// to make it thread safe.
unsafe impl Send for CallbackContext<FFISuccessCallbackContext> {}

// SAFETY: Allowing the void pointer contained within FFIFailureCallbackContext
// to be sent across threads should be safe.
// The void pointer is provided by the caller and it is the caller's responsibility
// to make it thread safe.
unsafe impl Send for CallbackContext<FFIFailureCallbackContext> {}

// SAFETY: Allowing the void pointer contained within FFISuccessCallbackContext
// to be sent across threads should be safe.
// The void pointer is provided by the caller and it is the caller's responsibility
// to make it thread safe.
unsafe impl Sync for CallbackContext<FFISuccessCallbackContext> {}

// SAFETY: Allowing the void pointer contained within FFIFailureCallbackContext
// to be sent across threads should be safe.
// The void pointer is provided by the caller and it is the caller's responsibility
// to make it thread safe.
unsafe impl Sync for CallbackContext<FFIFailureCallbackContext> {}

struct Dex2OatCallback {
    on_success_c_cb: FFIOnSuccessCallback,
    on_success_c_cb_ctx: CallbackContext<FFISuccessCallbackContext>,
    on_failure_c_cb: FFIOnFailureCallback,
    on_failure_c_cb_ctx: CallbackContext<FFIFailureCallbackContext>,
}

impl binder::Interface for Dex2OatCallback {}

impl IDex2OatTaskCallback for Dex2OatCallback {
    fn onSuccess(&self, metrics: &Dex2OatMetrics) -> binder::Result<()> {
        let result_ctx = SuccessResultContext {
            wall_time_ms: metrics.wallclock_time_milliseconds,
            cpu_time_ms: metrics.cpu_time_milliseconds,
        };
        let ffi_result_ctx = FFISuccessResultContext {
            ctx: (&result_ctx as *const SuccessResultContext) as *const c_void,
        };
        let cb = self.on_success_c_cb.as_ref().unwrap();
        // SAFETY: on_success_c_cb and cb_ctx are all checked during
        // AVerifiedDex2Oat_createCompilationContext for non-nullness. The callers are
        // required to pass in a valid pointer to an appropriate callback function
        // during context creation.
        unsafe { (cb)(self.on_success_c_cb_ctx.clone().cb_context, ffi_result_ctx) };
        Ok(())
    }

    fn onFailure(&self, failure_reason: Dex2OatFailureReason, message: &str) -> binder::Result<()> {
        let failure_code: FFIFailureReason = from_dex2oat_failure_reason(failure_reason);
        let cstr_message = CString::new(message).unwrap();
        let failure_result_ctx = FailureResultContext { failure_code, message: cstr_message };
        let result_ctx = FFIFailureResultContext {
            ctx: (&failure_result_ctx as *const FailureResultContext) as *const c_void,
        };

        let cb = self.on_failure_c_cb.as_ref().unwrap();
        // SAFETY: on_failure_c_cb and cb_ctx are all checked during
        // AVerifiedDex2Oat_createCompilationContext for non-nullness. The callers are
        // required to pass in a valid pointer to an appropriate callback function
        // during context creation.
        unsafe { (cb)(self.on_failure_c_cb_ctx.clone().cb_context, result_ctx) };
        Ok(())
    }
}

/// Holds the state and resources for a single dex2oat compilation.
///
/// This struct is created by `AVerifiedDex2Oat_createCompilationContext` and destroyed by
/// `AVerifiedDex2Oat_destroyCompilationContext`.
/// It encapsulates the connection to the isolated compilation service, callbacks for reporting
/// results, and any arguments and file descriptors associated with the compilation task.
/// The lifetime of this context is tied to the lifetime of the compilation it represents.
///
/// see `AVerifiedDex2Oat_createCompilationContext`
#[allow(dead_code)] // The fields contained within will be read by AVerifiedDex2Oat_start in the
                    // future, at which point this allow can be removed.
struct CompilationContext {
    dex2oat_callback: Option<Strong<dyn IDex2OatTaskCallback>>,
    cancellation_callback: Option<Strong<dyn ICompilationTask>>,
    service: Strong<dyn IIsolatedCompilationService>,
    // Binder dex2oat arguments are stored in context to extend the lifetimes of the owned file
    // descriptors.
    args: Vec<BnDex2OatArg>,
    // Stored in context to tie the lifetime of the owned fd to the lifetime of
    // the compilation context.
    recorded_compiler_args_fd: ParcelFileDescriptor,
}

/// Creates and initializes a compilation context for a dex2oat operation.
///
/// This function establishes a connection to the isolated compilation service (`composd`)
/// and prepares a context for a subsequent `startVerifiedDex2Oat` call. The context
/// holds the service connection, callbacks for success or failure, and other necessary
/// resources.
///
/// - `out_ctx`: A pointer to a `*mut c_void` that will be populated with the created context.
/// - `on_success_c_cb`: A callback function to be invoked upon successful compilation.
/// - `on_success_c_cb_ctx`: An opaque context pointer that will be passed to the `on_success_c_cb`
///   callback. This context pointer will be dereferenced on different threads.
/// - `on_failure_c_cb`: A callback function to be invoked upon compilation failure.
/// - `on_failure_c_cb_ctx`: An opaque context pointer that will be passed to the `on_failure_c_cb`
///   callback. This context pointer will be dereferenced on different threads.
/// - `recorded_compiler_args_fd`: A file descriptor to which compiler arguments will be written.
///   This file descriptor must be valid. The ownership of this file descriptor is taken by this
///   function and is considered owned by `out_ctx`. The file descriptor will be closed when
///   AVerifiedDex2Oat_destroyCompilationContext is called on `out_ctx`. After this call the caller
///   should avoid performing any other operations on this file descriptor.
/// - `timeout_seconds`: The timeout in seconds for connecting to the compilation service.
///
/// Returns `SUCCESS` if the context is created successfully, or
/// `ERROR_COMPOS_SERVICE_UNAVAILABLE` if the service cannot be reached.
///
/// # Safety
///  It is the caller's responsibility that `out_ctx` points to a pointer that can safely be
///  changed to point at an opaque context blob.
///
///  - `recorded_compiler_args_fd` must be a valid file descriptor to a file opened for read/write.
///    Ownership is transferred to `out_ctx` at function exit at which point the caller should
///    abstain from performing any more file operations on the descriptor.

#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_createCompilationContext(
    out_ctx: *mut FFICompilationContext,
    on_success_c_cb: FFIOnSuccessCallback,
    on_success_c_cb_ctx: FFISuccessCallbackContext,
    on_failure_c_cb: FFIOnFailureCallback,
    on_failure_c_cb_ctx: FFIFailureCallbackContext,
    recorded_compiler_args_fd: i32,
    timeout_seconds: u64,
) -> i32 {
    if on_success_c_cb.is_none()
        || on_failure_c_cb.is_none()
        || out_ctx.is_null()
    // SAFETY: out_ctx, once checked for nullness, is guaranteed to point to a valid
    // CompilationContext by virtue of API contract.
        || !((unsafe { *out_ctx }).ctx.is_null())
    {
        return FFIStatus_BAD_ARGS;
    }
    let svc_barrier = ServiceBarrier::new();
    let svc_barrier_clone = svc_barrier.clone();
    thread::spawn(move || match wait_for_composd_interface("android.system.composd") {
        Ok(svc) => {
            svc_barrier_clone.set_service_and_notify(svc);
        }
        Err(error) => {
            svc_barrier_clone.set_failure_and_notify(error);
        }
    });

    let service_result = svc_barrier.wait_for_service(Duration::from_secs(timeout_seconds));
    if service_result.is_err() {
        return FFIStatus_COMPOS_SERVICE_UNAVAILABLE;
    }
    let service = service_result.unwrap();
    let callback = Dex2OatCallback {
        on_success_c_cb,
        on_success_c_cb_ctx: CallbackContext { cb_context: on_success_c_cb_ctx },
        on_failure_c_cb,
        on_failure_c_cb_ctx: CallbackContext { cb_context: on_failure_c_cb_ctx },
    };
    let dex2oat_callback =
        Some(BnDex2OatTaskCallback::new_binder(callback, binder::BinderFeatures::default()));
    // SAFETY: `recorded_compiler_args_fd` is provided by the C caller, it is the caller's
    // responsibility to ensure that the file descriptor is valid. Ownership of the file
    // descriptor is transferred to the compilation context.
    let owned_recorded_compiler_args_fd =
        unsafe { OwnedFd::from_raw_fd(recorded_compiler_args_fd) };
    let boxed_context = Box::new(CompilationContext {
        args: Vec::new(),
        dex2oat_callback,
        cancellation_callback: None,
        service,
        recorded_compiler_args_fd: ParcelFileDescriptor::new(owned_recorded_compiler_args_fd),
    });
    // SAFETY: `out_ctx` a non null pointer to a compilation context where `ctx` is null.
    // The rust code allocates a new context and attaches it to this compilation context.
    // It is now the responsibility of the API user to call destroyCompilationContext
    // on the opaque struct to avoid leaks.
    unsafe {
        (*out_ctx).ctx = Box::into_raw(boxed_context) as *mut c_void;
    }
    FFIStatus_DEX2OAT_SUCCESS
}

struct SuccessResultContext {
    cpu_time_ms: i32,
    wall_time_ms: i32,
}

/// Extracts the wall time, in milliseconds, from an opaque result context.
///
/// - `success_result_ctx` An opaque success result context pointer. This must be the
///   `success_result_ctx` passed into the `AVerifiedDex2Oat_OnSuccessCallback` function.
/// - `cpu_time_ms`` The integer this points to will be set to the CPU time, in milliseconds, taken
///   to perform the compilation.
/// - `wall_time_ms` This will be set to the wall time, in milliseconds, taken to perform the
///   compilation.
///
/// Returns `AVERIFIED_DEX2OAT_SUCCESS` on success
///   `AVERIFIED_DEX2OAT_BAD_ARGS` when `wall_time_ms` or `success_result_ctx.ctx`
///    are null pointers.
///
/// # Safety
///  - `success_result_ctx.ctx` must point to a `SuccessResultContext`
///  - `wall_time_ms` must point to a i32
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_CompilationStats_getWallClockTimeMs(
    success_result_ctx: FFISuccessResultContext,
    wall_time_ms: *mut i32,
) -> FFIStatus {
    if wall_time_ms.is_null() || success_result_ctx.ctx.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: Caller guarantees that success_result_ctx points to the success_result_ctx passed to
    // the on_success C callback. This is in turn guaranteed to be a SuccessResultContext.
    let success_result = unsafe { &(*(success_result_ctx.ctx as *const SuccessResultContext)) };
    // SAFETY: Caller guarantees that cpu_time_ms and wall_time_ms both point to variables of
    // type int32_t.
    unsafe {
        *wall_time_ms = success_result.wall_time_ms;
    }
    FFIStatus_DEX2OAT_SUCCESS
}

/// Extracts the cpu time, in milliseconds, from an opaque result context.
///
/// - `success_result_ctx` An opaque success result context pointer.
/// - `cpu_time_ms`` The integer this points to will be set to the CPU time, in milliseconds, taken
///   to perform the compilation.
///
/// Returns `AVERIFIED_DEX2OAT_SUCCESS` on success
///   `AVERIFIED_DEX2OAT_BAD_ARGS` when `cpu_time_ms` or `success_result_ctx.ctx`
///    are null pointers.
///
/// # Safety
///  - `success_result_ctx.ctx` when non-null must point to a SuccessResultContext type.
///  - `cpu_time_ms` must point to a i32
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_CompilationStats_getCpuClockTimeMs(
    success_result_ctx: FFISuccessResultContext,
    cpu_time_ms: *mut i32,
) -> FFIStatus {
    if cpu_time_ms.is_null() || success_result_ctx.ctx.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: Caller guarantees that success_result_ctx points to the success_result_ctx passed to
    // the on_success C callback. This is in turn guaranteed to be a SuccessResultContext.
    let success_result = unsafe { &(*(success_result_ctx.ctx as *const SuccessResultContext)) };
    // SAFETY: Caller guarantees that cpu_time_ms and wall_time_ms both point to variables of
    // type int32_t.
    unsafe {
        *cpu_time_ms = success_result.cpu_time_ms;
    }
    FFIStatus_DEX2OAT_SUCCESS
}

struct FailureResultContext {
    failure_code: FFIFailureReason,
    message: CString,
}

/// Extracts the failure code from the opaque results context passed
/// into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// - `failure_result_ctx`` This is the opaque results context that is passed into the
///   `AVerifiedDex2Oat_OnFailureCallback`.
/// - `out_failure_code`` If the return code is `AVERIFIED_DEX2OAT_SUCCESS` this is set to a
///   `AVerifiedDex2Oat_Failure` code.
///
///  Returns: `AVerifiedDex2Oat_AVERIFIED_DEX2OAT_SUCCESS` on success
///  or `AVerifiedDex2Oat_AVERIFIED_DEX2OAT_BAD_ARGS` when `out_failure_code` or
///  `failure_result_ctx.ctx` are null.
///
/// # Safety
///   - `result_ctx` when non-null must point to a ResultContext type.
///   - `out_failure_code` must point to a `AVerifiedDex2Oat_FailureReason``.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureInfo_getFailureCode(
    failure_result_ctx: FFIFailureResultContext,
    out_failure_code: *mut FFIFailureReason,
) -> FFIStatus {
    if failure_result_ctx.ctx.is_null() || out_failure_code.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The caller guarantees that `failure_result_ctx` is the `failure_result_ctx`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_result = unsafe { &(*(failure_result_ctx.ctx as *const FailureResultContext)) };
    // SAFETY: The caller should guarantee that out_failure_code does point to a
    // `AVerifiedDex2Oat_FailureReason`
    unsafe { *out_failure_code = failure_result.failure_code };
    FFIStatus_DEX2OAT_SUCCESS
}

/// Extracts the failure code message from the opaque results context passed
/// into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// - `failure_result_ctx`` This is the opaque results context that is passed into the
///   `AVerifiedDex2Oat_OnFailureCallback`.
/// - `out_failure_message_ptr_ptr`` If the return code is `AVERIFIED_DEX2OAT_SUCCESS` then the
///   pointer this pointer points to will instead point to a null terminated message UTF-8 string.
///   The string that is pointed to will live until `AVerifiedDex2Oat_OnFailureCallback` exits.
///
///  Returns: `AVerifiedDex2Oat_AVERIFIED_DEX2OAT_SUCCESS` on success
///  or `AVerifiedDex2Oat_AVERIFIED_DEX2OAT_BAD_ARGS` when `out_failure_message_ptr_ptr` or
///  `failure_result_ctx.ctx` are null.
///
/// # Safety
///   - `result_ctx` when non-null must point to a ResultContext type.
///   - `out_failure_message_ptr_ptr` must point to a c-string pointer.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureInfo_getFailureMessage(
    failure_result_ctx: FFIFailureResultContext,
    out_failure_message_ptr_ptr: *mut *const c_char,
) -> FFIStatus {
    if failure_result_ctx.ctx.is_null() || out_failure_message_ptr_ptr.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The caller guarantees that `failure_result_ctx` is the `failure_result_ctx`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_result = unsafe { &(*(failure_result_ctx.ctx as *const FailureResultContext)) };
    // SAFETY: The caller should guarantee that out_failure_code does point to a
    // `AVerifiedDex2Oat_FailureReason`
    unsafe { *out_failure_message_ptr_ptr = failure_result.message.as_ptr() };
    FFIStatus_DEX2OAT_SUCCESS
}

/// Destroys a `CompilationContext` and frees associated resources.
///
/// This function takes a pointer to a `CompilationContext` and safely deallocates it.
///
/// - `compilation_context`: The opaque compilation context struct created by a previous call to
///   `AVerifiedDex2Oat_createCompilationContext`
///
/// Returns `SUCCESS` if the context was destroyed successfully, or `BAD_ARGS` if
/// `compilation_context.ctx` is null.
///
/// # Safety
///  - `compilation_context` MUST be a context created by AVerfieidDex2Oat_createCompilationContext,
///    that is the context was produced using a `Box::into_raw`
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_destroyCompilationContext(
    compilation_context: FFICompilationContext,
) -> i32 {
    if compilation_context.ctx.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The ctx contained within the compilation context is created by
    // `AVerifiedDex2Oat_createCompilationContext` via Box. This converts the raw
    // pointer back into a Box so that it can be deallocated.
    let _ = unsafe { Box::from_raw(compilation_context.ctx as *mut CompilationContext) };
    FFIStatus_DEX2OAT_SUCCESS
}

/// A synchronization primitive that allows one thread to wait for an asynchronous
/// service connection attempt by another thread.
///
/// It encapsulates a `Mutex`-protected `Option` that holds the result of the
/// connection attempt (either a `Strong` pointer to the service or an `Error`) and a `Condvar`
/// to signal completion.
struct ServiceBarrier {
    service: Mutex<Option<Result<Strong<dyn IIsolatedCompilationService>, Error>>>,
    cond: Condvar,
}

impl ServiceBarrier {
    /// Creates a new `ServiceBarrier` wrapped in an `Arc` for shared ownership.
    pub fn new() -> Arc<Self> {
        Arc::new(ServiceBarrier { service: Mutex::new(None), cond: Condvar::new() })
    }

    /// Waits for the service to become available or for the timeout to elapse.
    ///
    /// This method blocks the current thread until another thread calls `set_service_and_notify` or
    /// `set_failure_and_notify`, or until the specified `timeout` is reached.
    ///
    /// `timeout`: The maximum duration to wait for the service.
    ///
    /// Returns a `Result` containing the `Strong` pointer to the service on success, or an `Error`
    /// if the connection failed or timed out.
    pub fn wait_for_service(
        &self,
        timeout: Duration,
    ) -> Result<Strong<dyn IIsolatedCompilationService>, Error> {
        let guard = self.service.lock().map_err(|e| Error::msg(e.to_string()))?;
        let (mut service_guard, timeout_result) =
            self.cond.wait_timeout_while(guard, timeout, |service| service.is_none()).unwrap();
        if timeout_result.timed_out() {
            return Err(Error::msg("Timed out waiting for composd service"));
        }
        match service_guard.take().unwrap() {
            Ok(service) => Ok(service),
            Err(e) => Err(e),
        }
    }

    /// Sets the service result to a successful connection and notifies all waiting thread.
    ///
    /// This should be called by the thread that successfully established the service connection.
    pub fn set_service_and_notify(&self, service: Strong<dyn IIsolatedCompilationService>) {
        let (lock, cvar) = (&self.service, &self.cond);
        let mut svc = lock.lock().unwrap();
        *svc = Some(Ok(service));
        cvar.notify_all();
    }

    /// Sets the service result to a failure and notifies one waiting thread.
    ///
    /// This should be called by the thread that failed to establish the service connection.
    pub fn set_failure_and_notify(&self, error: Error) {
        let (lock, cvar) = (&self.service, &self.cond);
        let mut svc = lock.lock().unwrap();
        *svc = Some(Err(error));
        cvar.notify_one();
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::wrappers::mock_binder;
    use android_system_composd::aidl::android::system::composd::{
        IDex2OatTaskCallback::FailureReason::FailureReason as Dex2OatFailureReason,
        IIsolatedCompilationService::{
            BnIsolatedCompilationService, MockIIsolatedCompilationService,
        },
    };
    use mockall::predicate::{always as any, eq};
    use std::{ffi::CStr, os::fd::AsRawFd};
    use tempfile::tempfile;

    /// A mockable trait used to verify that the C-style callbacks (`on_success` and `on_failure`)
    /// are invoked with the correct arguments from within the Rust FFI layer.
    #[mockall::automock]
    trait ResultCallBackVerifierInterface {
        fn on_success(&self, cpu_time_ms: i32, wall_time_ms: i32);
        fn on_failure(&self, failure_reason: FFIFailureReason, message: &CStr);
    }

    /// C-style function pointer that acts as a bridge to the `on_success` method of the
    /// `MockResultCallBackVerifierInterface`.
    ///
    /// # Safety
    /// - `ctx` must be a valid pointer to a `MockResultCallBackVerifierInterface`
    /// - `stats` must be valid pointer to `CompilationStats`.
    unsafe extern "C" fn on_success_fn_ptr(
        cb_ctx: FFISuccessCallbackContext,
        result_ctx: FFISuccessResultContext,
    ) {
        assert!(!cb_ctx.ctx.is_null());
        assert!(!result_ctx.ctx.is_null());
        // SAFETY: Unit test code, ctx is guaranteed to be the correct type.
        let mock = unsafe { &(*(cb_ctx.ctx as *const MockResultCallBackVerifierInterface)) };
        let mut cpu_time_ms: i32 = 0;
        let mut wall_time_ms: i32 = 0;
        // SAFETY: cpu_time_ms and wall_time_ms are both appropriately typed
        // result_ctx is guaranteed to be backed by a SuccessResultContext.
        unsafe {
            assert_eq!(
                AVerifiedDex2Oat_CompilationStats_getCpuClockTimeMs(result_ctx, &mut cpu_time_ms),
                FFIStatus_DEX2OAT_SUCCESS
            );
            assert_eq!(
                AVerifiedDex2Oat_CompilationStats_getWallClockTimeMs(result_ctx, &mut wall_time_ms),
                FFIStatus_DEX2OAT_SUCCESS
            );
        }
        mock.on_success(cpu_time_ms, wall_time_ms);
    }

    /// C-style function pointer that acts as a bridge to the `on_failure` method of the
    /// `MockResultCallBackVerifierInterface`.
    ///
    /// # Safety
    /// - `ctx` must be a valid pointer to `MockResultCallBackVerifierInterface`
    /// - `message` must be a valid pointer to a null-terminated C string.
    unsafe extern "C" fn on_failure_fn_ptr(
        cb_ctx: FFIFailureCallbackContext,
        result_ctx: FFIFailureResultContext,
    ) {
        assert!(!cb_ctx.ctx.is_null());
        assert!(!result_ctx.ctx.is_null());
        // SAFETY: Unit test code, cb_ctx is guaranteed to be the correct type.
        let mock = unsafe { &(*(cb_ctx.ctx as *const MockResultCallBackVerifierInterface)) };
        let mut failure_code: FFIFailureReason = FFIFailureReason_UNKNOWN;
        let dummy_string: CString = CString::new("hello").unwrap();
        let mut c_char_ptr: *const c_char = dummy_string.as_ptr();
        // SAFETY: result_ctx is guaranteed to be backed by a FailureResultContext
        // failure_code and c_char_ptr are valid, see previous lines of code.
        unsafe {
            assert_eq!(
                AVerifiedDex2Oat_FailureInfo_getFailureCode(result_ctx, &mut failure_code,),
                FFIStatus_DEX2OAT_SUCCESS
            );
            assert_eq!(
                AVerifiedDex2Oat_FailureInfo_getFailureMessage(result_ctx, &mut c_char_ptr,),
                FFIStatus_DEX2OAT_SUCCESS
            );
        };
        // SAFETY: c_char_ptr is set to point to a valid C string by
        // `AVerifiedDex2Oat_extractFailureCodeAndMessageFromResultContext`
        let message: &CStr = unsafe { CStr::from_ptr(c_char_ptr) };
        mock.on_failure(failure_code, message);
    }

    /// Tests the successful creation and destruction of a `CompilationContext`.
    /// It verifies that:
    ///  1. `AVerifiedDex2Oat_createCompilationContext` successfully connects to the mock `composd`
    ///     service.
    ///  2. The returned context pointer is valid and properly initialized.
    ///  3. The provided C callbacks can be successfully invoked from the Rust side.
    ///  4. `AVerifiedDex2Oat_destroyCompilationContext` successfully deallocates the context.
    #[test]
    fn test_compile_context_create_success() {
        let wait_for_composd_ctx = mock_binder::wait_for_composd_interface_context();
        wait_for_composd_ctx
            .expect()
            .withf(|name| name == "android.system.composd")
            .times(1)
            .returning(move |_| {
                let mut mock = MockIIsolatedCompilationService::default();
                mock.expect_startVerifiedDex2Oat().never();
                Ok(BnIsolatedCompilationService::new_binder(
                    mock,
                    binder::BinderFeatures::default(),
                ))
            });

        let metrics =
            Dex2OatMetrics { wallclock_time_milliseconds: 123, cpu_time_milliseconds: 321 };
        let expected_failure_message = "failure_message";
        let expected_failure_reason = FFIFailureReason_COMPILATION_SETUP_FAILED;

        let mut opaque_comp_ctx = FFICompilationContext { ctx: std::ptr::null_mut() };
        let mut mock_cb_verifier = MockResultCallBackVerifierInterface::new();

        let expected_cpu_time = metrics.cpu_time_milliseconds;
        let expected_wall_time = metrics.wallclock_time_milliseconds;
        mock_cb_verifier
            .expect_on_success()
            .with(eq(expected_cpu_time), eq(expected_wall_time))
            .return_once(|_, _| ());
        mock_cb_verifier
            .expect_on_failure()
            .with(eq(expected_failure_reason), any())
            .return_once(|_, _| ());
        let success_cb_ctx = FFISuccessCallbackContext {
            ctx: (&mut mock_cb_verifier as *mut MockResultCallBackVerifierInterface) as *mut c_void,
        };
        let failure_cb_ctx = FFIFailureCallbackContext {
            ctx: (&mut mock_cb_verifier as *mut MockResultCallBackVerifierInterface) as *mut c_void,
        };
        let recorded_args_file = tempfile().unwrap();
        let recorded_args_fd = recorded_args_file.as_raw_fd();
        // SAFETY: out_ctx_ptr_ptr is a pointer to a valid pointer (which points at nothing).
        //  - on_success_fn_ptr is a function of type OnSuccessCallback with static lifetime
        //  - on_failure_fn_ptr is a function of type OnFailureCallback with static lifetime.
        //  - cb_ctx is a valid pointer created by Box::into_raw with a lifetime of this test.
        //  - recorded_args_fd - is a raw_fd of a File created by tempfile(). the lifetime of the
        //    File is the lifetime of this test.
        let result = unsafe {
            AVerifiedDex2Oat_createCompilationContext(
                &mut opaque_comp_ctx,
                Some(on_success_fn_ptr),
                success_cb_ctx,
                Some(on_failure_fn_ptr),
                failure_cb_ctx,
                recorded_args_fd,
                1, // timeout in seconds
            )
        };

        assert_eq!(result, FFIStatus_DEX2OAT_SUCCESS);
        // SAFETY: opaque_comp_ctx.ctx is set to point to a Boxed CompilationContext by
        // AVerifiedDex2Oat_createCompilationContext.
        let comp_ctx = unsafe { &*(opaque_comp_ctx.ctx as *const CompilationContext) };

        assert_eq!(comp_ctx.cancellation_callback, None);
        assert_eq!(comp_ctx.recorded_compiler_args_fd.as_raw_fd(), recorded_args_fd);
        assert_ne!(comp_ctx.dex2oat_callback, None);
        let dex2oat_cb = comp_ctx.dex2oat_callback.as_ref().unwrap();

        assert!(dex2oat_cb.onSuccess(&metrics).is_ok());
        assert!(dex2oat_cb
            .onFailure(Dex2OatFailureReason::CompilationSetupFailed, expected_failure_message)
            .is_ok());

        // Clean up the created context.
        // SAFETY: `opaque_comp_ctx` was initialized by `AVerifiedDex2Oat_createCompilationContext`,
        // satisfying the safety requirements of `AVerifiedDex2Oat_destroyCompilationContext`.
        let destroy_result = unsafe { AVerifiedDex2Oat_destroyCompilationContext(opaque_comp_ctx) };
        assert_eq!(destroy_result, FFIStatus_DEX2OAT_SUCCESS);
    }
}
