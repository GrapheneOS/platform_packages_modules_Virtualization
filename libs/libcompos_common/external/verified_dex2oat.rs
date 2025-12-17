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
        FailureDetails::FailureDetails as Dex2OatFailureDetails,
        FailureReason::FailureReason as Dex2OatFailureReason, IDex2OatTaskCallback,
    },
    IIsolatedCompilationService::{
        Dex2OatArg::Dex2OatArg as BnDex2OatArg, IIsolatedCompilationService,
    },
};
use anyhow::Error;
use binder::{ExceptionCode as BnExceptionCode, ParcelFileDescriptor, Status as BnStatus, Strong};
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
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_BAD_ARGS_FORMAT_STRING_NOT_UTF8 as FFIStatus_BAD_ARGS_FORMAT_STRING_NOT_UTF8,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS as FFIStatus_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_CTX_MISSING_ARGS as FFIStatus_CTX_MISSING_ARGS,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE as FFIStatus_CTX_UNEXPECTED_COMPILATION_STATE,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_ERROR_CALLING_COMPOS as FFIStatus_ERROR_CALLING_COMPOS,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE as FFIStatus_COMPOS_SERVICE_UNAVAILABLE,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_ERROR_GENERAL as FFIStatus_ERROR_GENERAL,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_ERROR_TIMED_OUT as FFIStatus_ERROR_TIMED_OUT,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_SUCCESS as FFIStatus_SUCCESS,
    AVerifiedDex2Oat_SuccessCallbackContext as FFISuccessCallbackContext,
    AVerifiedDex2Oat_SuccessResultContext as FFISuccessResultContext,
};
use parking_lot::{Condvar, Mutex};
use std::{
    ffi::{c_char, c_int, CStr, CString},
    marker::{Send, Sync},
    os::fd::BorrowedFd,
    slice,
    sync::Arc,
    thread,
    time::Duration,
};

const COMPILATION_STATE_MUTEX_TIMEOUT: Duration = Duration::from_millis(500);

// Returns the number of unescaped `!` in a string.
fn count_placeholders(fmt_str: &str) -> u32 {
    let mut placeholder_count = 0;
    let mut escaped = false;
    for c in fmt_str.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
        } else if c == '!' {
            placeholder_count += 1;
        }
    }
    placeholder_count
}

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
    cb_context: *mut T,
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
    compilation_state: Arc<Mutex<CompilationState>>,
}

impl binder::Interface for Dex2OatCallback {}

impl IDex2OatTaskCallback for Dex2OatCallback {
    fn onSuccess(&self, metrics: &Dex2OatMetrics) -> binder::Result<()> {
        if self.on_success_c_cb.is_none() {
            return Ok(());
        }
        let guard_result = self.compilation_state.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
        if guard_result.is_none() {
            return Err(BnStatus::new_exception(
                BnExceptionCode::SERVICE_SPECIFIC,
                Some(c"Timed out while checking compilation state"),
            ));
        }
        let mut guard = guard_result.unwrap();
        if *guard != CompilationState::Started {
            return Err(BnStatus::new_exception(
                BnExceptionCode::SERVICE_SPECIFIC,
                Some(c"Compilation was likely already canceled"),
            ));
        }
        let result_ctx = SuccessResultContext {
            wall_time_ms: metrics.wallclock_time_milliseconds,
            cpu_time_ms: metrics.cpu_time_milliseconds,
        };
        let ffi_result_ctx =
            (&result_ctx as *const SuccessResultContext) as *const FFISuccessResultContext;
        let cb = self.on_success_c_cb.as_ref().unwrap();
        // SAFETY: on_success_c_cb and cb_ctx are all checked during
        // AVerifiedDex2Oat_createCompilationContext for non-nullness. The callers are
        // required to pass in a valid pointer to an appropriate callback function
        // during context creation.
        unsafe { (cb)(self.on_success_c_cb_ctx.clone().cb_context, ffi_result_ctx) };
        *guard = CompilationState::Success;
        Ok(())
    }

    fn onFailure(&self, details: &Dex2OatFailureDetails) -> binder::Result<()> {
        if self.on_failure_c_cb.is_none() {
            return Ok(());
        }
        let guard_result = self.compilation_state.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
        if guard_result.is_none() {
            return Err(BnStatus::new_exception(
                BnExceptionCode::SERVICE_SPECIFIC,
                Some(c"Timed out while checking compilation state"),
            ));
        }
        let mut guard = guard_result.unwrap();
        if *guard != CompilationState::Started {
            return Err(BnStatus::new_exception(
                BnExceptionCode::SERVICE_SPECIFIC,
                Some(c"Compilation was likely already canceled"),
            ));
        }
        let failure_result_ctx = FailureResultContext {
            reason: from_dex2oat_failure_reason(details.reason),
            exit_code: details.exit_code,
            cpu_time: details.cpu_time_milliseconds,
            wall_time: details.wallclock_time_milliseconds,
            message: CString::new(details.message.clone()).unwrap(),
        };
        let result_ctx =
            (&failure_result_ctx as *const FailureResultContext) as *const FFIFailureResultContext;

        let cb = self.on_failure_c_cb.as_ref().unwrap();
        // SAFETY: on_failure_c_cb and cb_ctx are all checked during
        // AVerifiedDex2Oat_createCompilationContext for non-nullness. The callers are
        // required to pass in a valid pointer to an appropriate callback function
        // during context creation.
        unsafe { (cb)(self.on_failure_c_cb_ctx.clone().cb_context, result_ctx) };
        *guard = CompilationState::Failed;
        Ok(())
    }
}

#[derive(PartialEq)]
enum CompilationState {
    Idle,
    Started,
    Failed,
    Success,
    Canceled,
    Destroyed,
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
    dex2oat_callback: Strong<dyn IDex2OatTaskCallback>,
    cancellation_callback: Option<Strong<dyn ICompilationTask>>,
    service: Strong<dyn IIsolatedCompilationService>,
    // Binder dex2oat arguments are stored in context to extend the lifetimes of the owned file
    // descriptors.
    args: Vec<BnDex2OatArg>,
    // Stored in context to tie the lifetime of the owned fd to the lifetime of
    // the compilation context.
    recorded_compiler_args_fd: ParcelFileDescriptor,
    state: Arc<Mutex<CompilationState>>,
}

/// Creates and initializes a compilation context for a dex2oat operation.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///  It is the caller's responsibility that `out_ctx` points to a pointer that can safely be
///  changed to point at an opaque context blob.
///
///  - `recorded_compiler_args_fd` must be a valid file descriptor to a file opened for read/write.
///    This fd will be duplicated, the owner should refrain from writing to the fd until compilation
///    is finished.

#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_createCompilationContext(
    out_ctx_ptr_ptr: *mut *mut FFICompilationContext,
    on_success_c_cb: FFIOnSuccessCallback,
    on_success_c_cb_ctx: *mut FFISuccessCallbackContext,
    on_failure_c_cb: FFIOnFailureCallback,
    on_failure_c_cb_ctx: *mut FFIFailureCallbackContext,
    recorded_compiler_args_fd: i32,
    timeout_seconds: u64,
) -> i32 {
    if out_ctx_ptr_ptr.is_null() || on_success_c_cb.is_none() || on_failure_c_cb.is_none() {
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
    let state = Arc::new(Mutex::new(CompilationState::Idle));
    let callback = Dex2OatCallback {
        on_success_c_cb,
        on_success_c_cb_ctx: CallbackContext { cb_context: on_success_c_cb_ctx },
        on_failure_c_cb,
        on_failure_c_cb_ctx: CallbackContext { cb_context: on_failure_c_cb_ctx },
        compilation_state: state.clone(),
    };
    let dex2oat_callback =
        BnDex2OatTaskCallback::new_binder(callback, binder::BinderFeatures::default());
    // SAFETY: `recorded_compiler_args_fd` is provided by the C caller, it is the caller's
    // responsibility to ensure that the file descriptor is valid.
    let borrowed_args_fd = unsafe { BorrowedFd::borrow_raw(recorded_compiler_args_fd) };
    let boxed_context = Box::new(CompilationContext {
        args: Vec::new(),
        dex2oat_callback,
        cancellation_callback: None,
        service,
        // Duplicate the fd and turn it into a parcel fd.
        recorded_compiler_args_fd: ParcelFileDescriptor::new(
            borrowed_args_fd.try_clone_to_owned().unwrap(),
        ),
        state,
    });
    // SAFETY: `out_ctx` a non null pointer to a compilation context where `ctx` is null.
    // The rust code allocates a new context and attaches it to this compilation context.
    // It is now the responsibility of the API user to call destroyCompilationContext
    // on the opaque struct to avoid leaks.
    unsafe {
        *out_ctx_ptr_ptr = Box::into_raw(boxed_context) as *mut FFICompilationContext;
    }
    FFIStatus_SUCCESS
}

/// Add a single dex2oat argument to the compilation context.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
/// - `compilation_ctx` must be a compilation context produced by
///   `AVerifiedDex2Oat_createCompilationContext`.
/// - `format_string` must be a UTF-8 null-terminated string.
/// - `fds` must point to a contiguous array of c_int, each entry must correspond to a valid, open,
///   file descriptor. The caller must relinquish ownership of these file descriptors after calling
///   this function.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_addArgToCompilationContext(
    compilation_ctx: *mut FFICompilationContext,
    format_string: *const c_char,
    fds: *const c_int,
    fd_count: u32,
) -> i32 {
    // SAFETY: The caller guarantees that `fds` points to a valid array of `c_int`
    // file descriptors with `fd_count` elements.
    let fds_slice =
        unsafe { slice::from_raw_parts(fds as *const c_int, fd_count.try_into().unwrap()) };

    let mut inner_fds: Vec<ParcelFileDescriptor> = Vec::new();
    for fd in fds_slice {
        // SAFETY: For F_GETFD any value of fd should be safe since an invalid file descriptor will
        // result in a `-1` return value.
        if unsafe { libc::fcntl(*fd, libc::F_GETFD) == -1 } {
            return FFIStatus_BAD_ARGS;
        }
        // SAFETY: The caller guarantees that `fd` is a valid and open file descriptor.
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(*fd) };
        // Duplicate the file descriptor and turn the new duplicate fd into a parcel fd.
        inner_fds.push(ParcelFileDescriptor::new(borrowed_fd.try_clone_to_owned().unwrap()));
    }

    if compilation_ctx.is_null() || format_string.is_null() {
        return FFIStatus_BAD_ARGS;
    }

    // SAFETY: `compilation_ctx` is guaranteed to be a valid CompilationContext produced by
    // `AVerifiedDex2Oat_createCompilationContext`.
    let comp_ctx = unsafe { &mut *(compilation_ctx as *mut CompilationContext) };
    let guard_result = comp_ctx.state.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
    if guard_result.is_none() {
        return FFIStatus_ERROR_TIMED_OUT;
    }
    let guard = guard_result.unwrap();
    if *guard != CompilationState::Idle {
        return FFIStatus_CTX_UNEXPECTED_COMPILATION_STATE;
    }

    // SAFETY: `format_string` is _Nonnullable and is specified to be a UTF-8 null terminated
    // string.
    let fmt_str = match unsafe { CStr::from_ptr(format_string) }.to_str() {
        Ok(s) => s,
        Err(_) => return FFIStatus_BAD_ARGS_FORMAT_STRING_NOT_UTF8,
    };

    let placeholder_count = count_placeholders(fmt_str);
    if placeholder_count != fd_count {
        return FFIStatus_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS;
    }

    if fd_count == 0 {
        comp_ctx.args.push(BnDex2OatArg { formatString: fmt_str.to_owned(), fds: inner_fds });
        return FFIStatus_SUCCESS;
    }

    comp_ctx.args.push(BnDex2OatArg { formatString: fmt_str.to_owned(), fds: inner_fds });
    FFIStatus_SUCCESS
}

struct SuccessResultContext {
    cpu_time_ms: i32,
    wall_time_ms: i32,
}

/// Extracts the wall time, in milliseconds, from an opaque result context.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///  - `success_result_ctx` must point to a `SuccessResultContext`
///  - `wall_time_ms` must point to a i32
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_CompilationStats_getWallClockTimeMs(
    success_result_ctx: *const FFISuccessResultContext,
    wall_time_ms: *mut i32,
) -> FFIStatus {
    if wall_time_ms.is_null() || success_result_ctx.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: Caller guarantees that success_result_ctx points to the success_result_ctx passed to
    // the on_success C callback. This is in turn guaranteed to be a SuccessResultContext.
    let success_result = unsafe { &(*(success_result_ctx as *const SuccessResultContext)) };
    // SAFETY: Caller guarantees that wall_time_ms point to variables of
    // type int32_t.
    unsafe {
        *wall_time_ms = success_result.wall_time_ms;
    }
    FFIStatus_SUCCESS
}

/// Extracts the cpu time, in milliseconds, from an opaque result context.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///  - `success_result_ctx` when non-null must point to a SuccessResultContext type.
///  - `cpu_time_ms` must point to a i32
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_CompilationStats_getCpuClockTimeMs(
    success_result_ctx: *const FFISuccessResultContext,
    cpu_time_ms: *mut i32,
) -> FFIStatus {
    if cpu_time_ms.is_null() || success_result_ctx.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: Caller guarantees that success_result_ctx points to the success_result_ctx passed to
    // the on_success C callback. This is in turn guaranteed to be a SuccessResultContext.
    let success_result = unsafe { &*(success_result_ctx as *const SuccessResultContext) };
    // SAFETY: Caller guarantees that cpu_time_ms and wall_time_ms both point to variables of
    // type int32_t.
    unsafe {
        *cpu_time_ms = success_result.cpu_time_ms;
    }
    FFIStatus_SUCCESS
}

struct FailureResultContext {
    reason: FFIFailureReason,
    exit_code: i32,
    cpu_time: i32,
    wall_time: i32,
    message: CString,
}

/// Extracts the failure code from the opaque results context passed
/// into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `result_ctx` when non-null must point to a ResultContext type.
///   - `out_failure_reason` must point to a `AVerifiedDex2Oat_FailureReason``.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureInfo_getReason(
    failure_result_ctx: *const FFIFailureResultContext,
    out_failure_reason: *mut FFIFailureReason,
) -> FFIStatus {
    if failure_result_ctx.is_null() || out_failure_reason.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The caller guarantees that `failure_result_ctx` is the `failure_result_ctx`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_details = unsafe { &*(failure_result_ctx as *const FailureResultContext) };
    // SAFETY: The caller should guarantee that out_failure_code does point to a
    // `AVerifiedDex2Oat_FailureReason`
    unsafe { *out_failure_reason = failure_details.reason };
    FFIStatus_SUCCESS
}

/// Extracts the failure exit code from the opaque results context passed
/// into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `result_ctx` when non-null must point to a ResultContext type.
///   - `out_failure_exit_code` must point to an `i32`.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureInfo_getExitCode(
    failure_result_ctx: *const FFIFailureResultContext,
    out_failure_exit_code: *mut i32,
) -> FFIStatus {
    if failure_result_ctx.is_null() || out_failure_exit_code.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The caller guarantees that `failure_result_ctx` is the `failure_result_ctx`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_details = unsafe { &*(failure_result_ctx as *const FailureResultContext) };
    // SAFETY: The caller should guarantee that out_failure_code does point to an `i32`
    unsafe { *out_failure_exit_code = failure_details.exit_code };
    FFIStatus_SUCCESS
}

/// Extracts the amount of CPU time spent on compilation before the failure occurred
/// from the opaque results context passed into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `result_ctx` when non-null must point to a ResultContext type.
///   - `out_failure_cpu_time` must point to an `i32`.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureInfo_getCpuClockTimeMs(
    failure_result_ctx: *const FFIFailureResultContext,
    out_failure_cpu_time: *mut i32,
) -> FFIStatus {
    if failure_result_ctx.is_null() || out_failure_cpu_time.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The caller guarantees that `failure_result_ctx` is the `failure_result_ctx`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_details = unsafe { &*(failure_result_ctx as *const FailureResultContext) };
    // SAFETY: The caller should guarantee that out_failure_cpu_time points to an `i32`
    unsafe { *out_failure_cpu_time = failure_details.cpu_time };
    FFIStatus_SUCCESS
}

/// Extracts the wallclock time spent on compilation before a failure occurred
/// from the opaque results context passed into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `result_ctx` when non-null must point to a ResultContext type.
///   - `out_failure_wall_time` must point to an `i32`.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureInfo_getWallClockTimeMs(
    failure_result_ctx: *const FFIFailureResultContext,
    out_failure_wall_time: *mut i32,
) -> FFIStatus {
    if failure_result_ctx.is_null() || out_failure_wall_time.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The caller guarantees that `failure_result_ctx` is the `failure_result_ctx`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_details = unsafe { &*(failure_result_ctx as *const FailureResultContext) };
    // SAFETY: The caller should guarantee that out_failure_wall_time points to an `i32`
    unsafe { *out_failure_wall_time = failure_details.wall_time };
    FFIStatus_SUCCESS
}

/// Extracts the failure code message from the opaque results context passed
/// into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `result_ctx` when non-null must point to a ResultContext type.
///   - `out_failure_message_ptr_ptr` must point to a c-string pointer.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureInfo_getMessage(
    failure_result_ctx: *const FFIFailureResultContext,
    out_failure_message_ptr_ptr: *mut *const c_char,
) -> FFIStatus {
    if failure_result_ctx.is_null() || out_failure_message_ptr_ptr.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The caller guarantees that `failure_result_ctx` is the `failure_result_ctx`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_result = unsafe { &*(failure_result_ctx as *const FailureResultContext) };
    // SAFETY: The caller should guarantee that out_failure_code does point to a
    // `AVerifiedDex2Oat_FailureReason`
    unsafe { *out_failure_message_ptr_ptr = failure_result.message.as_ptr() };
    FFIStatus_SUCCESS
}

/// Destroys a `CompilationContext` and frees associated resources.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///  - `compilation_context` MUST be a context created by AVerfieidDex2Oat_createCompilationContext,
///    that is the context was produced using a `Box::into_raw`
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_destroyCompilationContext(
    compilation_context: *mut FFICompilationContext,
) -> i32 {
    if compilation_context.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The ctx contained within the compilation context is created by
    // `AVerifiedDex2Oat_createCompilationContext` via Box. This converts the raw
    // pointer back into a Box so that it can be deallocated.
    let ctx = unsafe { Box::from_raw(compilation_context as *mut CompilationContext) };
    let guard_result = ctx.state.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
    if guard_result.is_none() {
        return FFIStatus_ERROR_TIMED_OUT;
    }
    let mut guard = guard_result.unwrap();
    *guard = CompilationState::Destroyed;
    FFIStatus_SUCCESS
}

/// Starts a dex2oat compilation within a VM.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
/// The caller must guarantee:
/// - `compilation_ctx` was created using `AVerifiedDex2Oat_createCompilationContext`
/// - No other process is concurrently accessing `compilation_ctx` for the duration of this function
///   call.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_start(
    compilation_ctx: *mut FFICompilationContext,
    timeout_seconds: u32,
) -> i32 {
    if compilation_ctx.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The caller guarantees that `ctx` is a valid pointer to a `CompilationContext`.
    // We are dereferencing it to access the context.
    let comp_ctx = unsafe { &mut *(compilation_ctx as *mut CompilationContext) };
    if timeout_seconds == 0 {
        return FFIStatus_BAD_ARGS;
    }
    let guard_result = comp_ctx.state.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
    if guard_result.is_none() {
        return FFIStatus_ERROR_TIMED_OUT;
    }
    let mut guard = guard_result.unwrap();
    if *guard != CompilationState::Idle {
        return FFIStatus_CTX_UNEXPECTED_COMPILATION_STATE;
    }
    if comp_ctx.args.is_empty() {
        return FFIStatus_CTX_MISSING_ARGS;
    }

    let svc = comp_ctx.service.as_ref();
    let compiler_args_fd: &mut ParcelFileDescriptor = &mut comp_ctx.recorded_compiler_args_fd;
    match svc.startVerifiedDex2Oat(
        &comp_ctx.args,
        compiler_args_fd,
        &comp_ctx.dex2oat_callback,
        timeout_seconds.try_into().unwrap(),
    ) {
        Err(_) => {
            return FFIStatus_ERROR_CALLING_COMPOS;
        }
        Ok(cb) => {
            comp_ctx.cancellation_callback = Some(cb);
            *guard = CompilationState::Started;
        }
    }
    FFIStatus_SUCCESS
}

/// Cancels an ongoing dex2oat compilation.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
/// The caller must guarantee:
/// - `compilation_ctx` is a valid context created by `AVerifiedDex2Oat_createCompilationContext`
/// - No other process is concurrently accessing `compilation_ctx` for the duration of this function
///   call.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_cancel(
    compilation_ctx: *const FFICompilationContext,
) -> i32 {
    if compilation_ctx.is_null() {
        return FFIStatus_BAD_ARGS;
    }
    // SAFETY: The caller guarantees that `ctx` is a valid pointer to a `CompilationContext`.
    // We are dereferencing it to access the context.
    let comp_ctx = unsafe { &*(compilation_ctx as *const CompilationContext) };
    let guard_result = comp_ctx.state.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
    if guard_result.is_none() {
        return FFIStatus_ERROR_TIMED_OUT;
    }
    let mut guard = guard_result.unwrap();
    match *guard {
        CompilationState::Started => {
            if let Some(cancellation_task) = &comp_ctx.cancellation_callback {
                let _ = cancellation_task.cancel();
                *guard = CompilationState::Canceled;
                return FFIStatus_SUCCESS;
            }
            FFIStatus_ERROR_GENERAL
        }
        _ => FFIStatus_CTX_UNEXPECTED_COMPILATION_STATE,
    }
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
        let mut guard = self.service.lock();
        let timeout_result =
            self.cond.wait_while_for(&mut guard, |service| service.is_none(), timeout);
        if timeout_result.timed_out() {
            return Err(Error::msg("Timed out waiting for composd service"));
        }
        match guard.take().unwrap() {
            Ok(service) => Ok(service),
            Err(e) => Err(e),
        }
    }

    /// Sets the service result to a successful connection and notifies all waiting thread.
    ///
    /// This should be called by the thread that successfully established the service connection.
    pub fn set_service_and_notify(&self, service: Strong<dyn IIsolatedCompilationService>) {
        let (lock, cvar) = (&self.service, &self.cond);
        let mut svc = lock.lock();
        *svc = Some(Ok(service));
        cvar.notify_all();
    }

    /// Sets the service result to a failure and notifies one waiting thread.
    ///
    /// This should be called by the thread that failed to establish the service connection.
    pub fn set_failure_and_notify(&self, error: Error) {
        let (lock, cvar) = (&self.service, &self.cond);
        let mut svc = lock.lock();
        *svc = Some(Err(error));
        cvar.notify_one();
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::wrappers::mock_binder;
    use android_system_composd::aidl::android::system::composd::{
        ICompilationTask::{BnCompilationTask, MockICompilationTask},
        IDex2OatTaskCallback::{
            FailureReason::FailureReason as Dex2OatFailureReason, MockIDex2OatTaskCallback,
        },
        IIsolatedCompilationService::{
            BnIsolatedCompilationService, MockIIsolatedCompilationService,
        },
    };
    use mockall::predicate::{always as any, eq};
    use std::{
        ffi::CStr,
        fs::File,
        os::fd::{AsRawFd, OwnedFd},
    };
    use tempfile::tempfile;
    fn build_comp_ctx() -> CompilationContext {
        CompilationContext {
            dex2oat_callback: BnDex2OatTaskCallback::new_binder(
                MockIDex2OatTaskCallback::new(),
                binder::BinderFeatures::default(),
            ),
            cancellation_callback: None,
            args: Vec::new(),
            recorded_compiler_args_fd: ParcelFileDescriptor::new(OwnedFd::from(
                tempfile().unwrap(),
            )),
            service: BnIsolatedCompilationService::new_binder(
                MockIIsolatedCompilationService::new(),
                binder::BinderFeatures::default(),
            ),
            state: Arc::new(Mutex::new(CompilationState::Idle)),
        }
    }

    // For a vector of files return a vector containing their file descriptors.
    fn files_as_fds(files: &[File]) -> Vec<i32> {
        files.iter().map(|file| file.as_raw_fd()).collect()
    }

    fn get_temp_file_vec(count: usize) -> Vec<File> {
        std::iter::repeat_with(|| tempfile().unwrap()).take(count).collect()
    }

    fn add_arg_to_compilation_context(
        ctx: &mut CompilationContext,
        format_string: &CString,
        fds: &[i32],
    ) -> i32 {
        let compilation_ctx = (ctx as *mut CompilationContext) as *mut FFICompilationContext;
        // SAFETY: `ctx_ptr` is defined above as a valid pointer to a CompilationContext
        // variable. `format_string`, `fds` are both guaranteed to be a UTF-8
        // encoded C-strings and vectors of i32s.
        unsafe {
            AVerifiedDex2Oat_addArgToCompilationContext(
                compilation_ctx,
                format_string.as_ptr(),
                fds.as_ptr(),
                fds.len().try_into().unwrap(),
            )
        }
    }

    fn start_compilation(ctx: &mut CompilationContext, timeout_seconds: u32) -> i32 {
        let compilation_ctx = (ctx as *mut CompilationContext) as *mut FFICompilationContext;
        // SAFETY: compilation_ctx contains a valid opaque pointer to a CompilationContext variable,
        // see above.
        unsafe { AVerifiedDex2Oat_start(compilation_ctx, timeout_seconds) }
    }

    fn create_compilation_context(
        out_ctx_ptr_ptr: *mut *mut FFICompilationContext,
        on_success_c_cb: FFIOnSuccessCallback,
        on_failure_c_cb: FFIOnFailureCallback,
        callbacks_ctx: Option<&mut MockResultCallBackVerifierInterface>,
        compiler_args_file: &File,
        timeout_seconds: u64,
    ) -> i32 {
        let compiler_args_fd = compiler_args_file.as_raw_fd();
        let mut success_cb_ctx: *mut FFISuccessCallbackContext = std::ptr::null_mut();
        let mut failure_cb_ctx: *mut FFIFailureCallbackContext = std::ptr::null_mut();
        if let Some(cb_ctx) = callbacks_ctx {
            let common_cb_ctx = cb_ctx as *mut MockResultCallBackVerifierInterface;
            success_cb_ctx = common_cb_ctx as *mut FFISuccessCallbackContext;
            failure_cb_ctx = common_cb_ctx as *mut FFIFailureCallbackContext;
        }
        // SAFETY: out_ctx_ptr_ptr is a pointer to a valid pointer (which points at nothing).
        //  - on_success_fn_ptr is a function of type OnSuccessCallback with static lifetime
        //  - on_failure_fn_ptr is a function of type OnFailureCallback with static lifetime.
        //  - cb_ctx is a valid pointer created by Box::into_raw with a lifetime of this test.
        //  - recorded_args_fd - is a raw_fd of a File created by tempfile(). the lifetime of the
        //    File is the lifetime of this test.
        unsafe {
            AVerifiedDex2Oat_createCompilationContext(
                out_ctx_ptr_ptr,
                on_success_c_cb,
                success_cb_ctx,
                on_failure_c_cb,
                failure_cb_ctx,
                compiler_args_fd,
                timeout_seconds,
            )
        }
    }

    /// A mockable trait used to verify that the C-style callbacks (`on_success` and `on_failure`)
    /// are invoked with the correct arguments from within the Rust FFI layer.
    #[mockall::automock]
    trait ResultCallBackVerifierInterface {
        fn on_success(&self, cpu_time_ms: i32, wall_time_ms: i32);
        fn on_failure(
            &self,
            failure_reason: FFIFailureReason,
            exit_code: i32,
            cpu_time_ms: i32,
            wall_time_ms: i32,
            message: &CStr,
        );
    }

    /// C-style function pointer that acts as a bridge to the `on_success` method of the
    /// `MockResultCallBackVerifierInterface`.
    ///
    /// # Safety
    /// - `ctx` must be a valid pointer to a `MockResultCallBackVerifierInterface`
    /// - `stats` must be valid pointer to `CompilationStats`.
    unsafe extern "C" fn on_success_fn_ptr(
        cb_ctx: *mut FFISuccessCallbackContext,
        result_ctx: *const FFISuccessResultContext,
    ) {
        assert!(!cb_ctx.is_null());
        assert!(!result_ctx.is_null());
        // SAFETY: Unit test code, ctx is guaranteed to be the correct type.
        let mock = unsafe { &*(cb_ctx as *const MockResultCallBackVerifierInterface) };
        let mut cpu_time_ms: i32 = 0;
        let mut wall_time_ms: i32 = 0;
        // SAFETY: cpu_time_ms and wall_time_ms are both appropriately typed
        // result_ctx is guaranteed to be backed by a SuccessResultContext.
        unsafe {
            assert_eq!(
                AVerifiedDex2Oat_CompilationStats_getCpuClockTimeMs(result_ctx, &mut cpu_time_ms),
                FFIStatus_SUCCESS
            );
            assert_eq!(
                AVerifiedDex2Oat_CompilationStats_getWallClockTimeMs(result_ctx, &mut wall_time_ms),
                FFIStatus_SUCCESS
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
        cb_ctx: *mut FFIFailureCallbackContext,
        result_ctx: *const FFIFailureResultContext,
    ) {
        assert!(!cb_ctx.is_null());
        assert!(!result_ctx.is_null());
        // SAFETY: Unit test code, cb_ctx is guaranteed to be the correct type.
        let mock = unsafe { &*(cb_ctx as *const MockResultCallBackVerifierInterface) };
        let mut failure_code: FFIFailureReason = FFIFailureReason_UNKNOWN;
        let dummy_string: CString = CString::new("hello").unwrap();
        let mut c_char_ptr: *const c_char = dummy_string.as_ptr();
        let mut cpu_time: i32 = 0;
        let mut wall_time: i32 = 0;
        let mut exit_code: i32 = 0;
        // SAFETY: result_ctx is guaranteed to be backed by a FailureResultContext
        // failure_code and c_char_ptr are valid, see previous lines of code.
        unsafe {
            assert_eq!(
                AVerifiedDex2Oat_FailureInfo_getReason(result_ctx, &mut failure_code,),
                FFIStatus_SUCCESS
            );
            assert_eq!(
                AVerifiedDex2Oat_FailureInfo_getExitCode(result_ctx, &mut exit_code,),
                FFIStatus_SUCCESS
            );
            assert_eq!(
                AVerifiedDex2Oat_FailureInfo_getCpuClockTimeMs(result_ctx, &mut cpu_time),
                FFIStatus_SUCCESS
            );
            assert_eq!(
                AVerifiedDex2Oat_FailureInfo_getWallClockTimeMs(result_ctx, &mut wall_time),
                FFIStatus_SUCCESS
            );
            assert_eq!(
                AVerifiedDex2Oat_FailureInfo_getMessage(result_ctx, &mut c_char_ptr,),
                FFIStatus_SUCCESS
            );
        };
        // SAFETY: c_char_ptr is set to point to a valid C string by
        // `AVerifiedDex2Oat_extractFailureCodeAndMessageFromResultContext`
        let message: &CStr = unsafe { CStr::from_ptr(c_char_ptr) };
        mock.on_failure(failure_code, exit_code, cpu_time, wall_time, message);
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

        let expected_failure_details: Dex2OatFailureDetails = Dex2OatFailureDetails {
            reason: Dex2OatFailureReason::CompilationSetupFailed,
            exit_code: -2,
            cpu_time_milliseconds: 456,
            wallclock_time_milliseconds: 654,
            message: "failure_message".to_string(),
        };

        let mut opaque_comp_ctx: *mut FFICompilationContext = std::ptr::null_mut();
        let mut mock_cb_verifier = MockResultCallBackVerifierInterface::new();

        let expected_cpu_time = metrics.cpu_time_milliseconds;
        let expected_wall_time = metrics.wallclock_time_milliseconds;
        mock_cb_verifier
            .expect_on_success()
            .with(eq(expected_cpu_time), eq(expected_wall_time))
            .return_once(|_, _| ());
        mock_cb_verifier
            .expect_on_failure()
            .with(
                eq(from_dex2oat_failure_reason(expected_failure_details.reason)),
                eq(expected_failure_details.exit_code),
                eq(expected_failure_details.cpu_time_milliseconds),
                eq(expected_failure_details.wallclock_time_milliseconds),
                any(),
            )
            .return_once(|_, _, _, _, _| ());
        let recorded_args_file = tempfile().unwrap();
        let result = create_compilation_context(
            &mut opaque_comp_ctx,
            Some(on_success_fn_ptr),
            Some(on_failure_fn_ptr),
            Some(&mut mock_cb_verifier),
            &recorded_args_file,
            1,
        );

        assert_eq!(result, FFIStatus_SUCCESS);
        // SAFETY: opaque_comp_ctx is set to point to a Boxed CompilationContext by
        // AVerifiedDex2Oat_createCompilationContext.
        let comp_ctx = unsafe { &*(opaque_comp_ctx as *const CompilationContext) };
        assert_eq!(comp_ctx.cancellation_callback, None);
        let dex2oat_cb = &comp_ctx.dex2oat_callback;
        assert!(fds_are_equivalent(
            comp_ctx.recorded_compiler_args_fd.as_raw_fd(),
            recorded_args_file.as_raw_fd()
        ));

        *(comp_ctx.state.lock()) = CompilationState::Started;
        assert!(dex2oat_cb.onSuccess(&metrics).is_ok());
        *(comp_ctx.state.lock()) = CompilationState::Started;
        assert!(dex2oat_cb.onFailure(&expected_failure_details).is_ok());

        // Clean up the created context.
        // SAFETY: `opaque_comp_ctx` was initialized by `AVerifiedDex2Oat_createCompilationContext`,
        // satisfying the safety requirements of `AVerifiedDex2Oat_destroyCompilationContext`.
        let destroy_result = unsafe { AVerifiedDex2Oat_destroyCompilationContext(opaque_comp_ctx) };
        assert_eq!(destroy_result, FFIStatus_SUCCESS);
    }

    #[test]
    fn test_compile_context_create_failure_no_callbacks() {
        let mut opaque_comp_ctx: *mut FFICompilationContext = std::ptr::null_mut();
        let recorded_args_file = tempfile().unwrap();
        let result = create_compilation_context(
            &mut opaque_comp_ctx,
            None,
            None,
            None,
            &recorded_args_file,
            1,
        );
        assert_eq!(result, FFIStatus_BAD_ARGS);
    }

    fn fds_are_equivalent(fd1: i32, fd2: i32) -> bool {
        // SAFETY: stat1 isn't used until filled by libc::stat.
        let mut stat1: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: stat2 isn't used until filled by libc::stat.
        let mut stat2: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: stat1 is a valid libc::stat variable.
        let mut result = unsafe { libc::fstat(fd1, &mut stat1 as *mut libc::stat) };
        assert_eq!(result, 0);
        // SAFETY: stat2 is a valid libc::stat variable.
        result = unsafe { libc::fstat(fd2, &mut stat2 as *mut libc::stat) };
        assert_eq!(result, 0);
        stat1.st_dev == stat2.st_dev && stat1.st_ino == stat2.st_ino
    }

    #[test]
    fn test_add_args_success() {
        let mut comp_ctx = build_comp_ctx();

        const ARG_COUNT: usize = 25;

        // Generate 25 format strings, each format string's placeholder count is equal to its index.
        let format_string: Vec<CString> = (0..ARG_COUNT)
            .map(|n| CString::new(format!("FormatString\\!fd={}", "!;".repeat(n))).unwrap())
            .collect();
        // Create a list-of-list of tempfiles corresponding to the format-strings
        let files: Vec<Vec<File>> = (0..ARG_COUNT).map(get_temp_file_vec).collect();

        // Generate a list of file descriptors from the list-of-list of files.
        let fds_list: Vec<Vec<i32>> = files.iter().map(|n| files_as_fds(n)).collect();

        let zipped = format_string.iter().zip(fds_list);
        for (format_str, fds) in zipped.clone() {
            assert_eq!(
                add_arg_to_compilation_context(&mut comp_ctx, format_str, &fds),
                FFIStatus_SUCCESS
            );
        }
        let args_match = comp_ctx.args.iter().zip(zipped).all(|(actual, expected)| {
            let format_str_match = actual.formatString.as_str() == expected.0.to_str().unwrap();
            let fds_match = actual
                .fds
                .iter()
                .zip(expected.1.iter())
                .all(|(parcel_fd, fd)| fds_are_equivalent(parcel_fd.as_raw_fd(), *fd));
            format_str_match && fds_match
        });
        assert!(args_match);
    }

    #[test]
    fn test_add_args_on_started_context_failure() {
        const ARG_COUNT: usize = 3;
        let mut comp_ctx = build_comp_ctx();
        *(comp_ctx.state.lock()) = CompilationState::Started;
        let mock_cancel_cb = BnCompilationTask::new_binder(
            MockICompilationTask::new(),
            binder::BinderFeatures::default(),
        );
        comp_ctx.cancellation_callback = Some(mock_cancel_cb);

        // Generate 3 format strings, each format string's placeholder count is equal to its index.
        let format_string: Vec<CString> = (0..ARG_COUNT)
            .map(|n| CString::new(format!("FormatString\\!fd={}", "!;".repeat(n))).unwrap())
            .collect();
        // Create a list-of-list of tempfiles corresponding to the format-strings
        let files: Vec<Vec<File>> = (0..ARG_COUNT).map(get_temp_file_vec).collect();

        // Generate a list of file descriptors from the list-of-list of files.
        let fds_list: Vec<Vec<i32>> = files.iter().map(|n| files_as_fds(n)).collect();

        let zipped = format_string.iter().zip(fds_list);
        for (format_str, fds) in zipped.clone() {
            assert_eq!(
                add_arg_to_compilation_context(&mut comp_ctx, format_str, &fds),
                FFIStatus_CTX_UNEXPECTED_COMPILATION_STATE
            );
        }
    }

    #[test]
    fn test_add_args_when_placeholder_count_ne_fd_count_failure() {
        let mut comp_ctx = build_comp_ctx();
        let format_str = CString::new("ThreePlaceholders!!!").unwrap();
        let file = tempfile().unwrap();
        let fds = [file.as_raw_fd()];
        assert_eq!(
            add_arg_to_compilation_context(&mut comp_ctx, &format_str, &fds),
            FFIStatus_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS
        );
    }

    #[test]
    fn test_add_args_with_bad_fds_failure() {
        let mut comp_ctx = build_comp_ctx();
        let format_str = CString::new("FormatString!").unwrap();
        let fds = [tempfile().unwrap().as_raw_fd()];
        assert_eq!(
            add_arg_to_compilation_context(&mut comp_ctx, &format_str, &fds),
            FFIStatus_BAD_ARGS
        );
    }

    #[test]
    fn test_compile_start_dex2oat_success() {
        const ARG_COUNT: usize = 5;
        const FD_COUNT_PER_ARG: u32 = 3;
        struct TestArgs {
            format_string: String,
            fd_raw: Vec<i32>,
        }

        let dex2oat_args: Vec<BnDex2OatArg> = (0..ARG_COUNT)
            .map(|index| BnDex2OatArg {
                formatString: format!("FormatString{}", "!".repeat(index)),
                fds: (0..FD_COUNT_PER_ARG)
                    .map(|_| ParcelFileDescriptor::new::<OwnedFd>(tempfile().unwrap().into()))
                    .collect(),
            })
            .collect();
        let expected_args: Vec<TestArgs> = dex2oat_args
            .iter()
            .map(|arg| TestArgs {
                format_string: arg.formatString.clone(),
                fd_raw: arg.fds.iter().map(|fd| fd.as_raw_fd()).collect(),
            })
            .collect();
        let metrics =
            Dex2OatMetrics { wallclock_time_milliseconds: 123, cpu_time_milliseconds: 321 };
        let expected_failure_details: Dex2OatFailureDetails = Dex2OatFailureDetails {
            reason: Dex2OatFailureReason::Timeout,
            exit_code: -2,
            cpu_time_milliseconds: 456,
            wallclock_time_milliseconds: 654,
            message: "failure_message".to_string(),
        };
        let mut mock_dex_cb = MockIDex2OatTaskCallback::new();
        mock_dex_cb.expect_onSuccess().with(eq(metrics)).once().return_once(|_| Ok(()));
        let details_clone = expected_failure_details.clone();
        mock_dex_cb
            .expect_onFailure()
            .withf(move |in_details| *in_details == details_clone)
            .times(1)
            .return_once(|_| Ok(()));
        let mut mock_dex2oat_svc = MockIIsolatedCompilationService::new();
        let recorded_compiler_args_file =
            ParcelFileDescriptor::new(OwnedFd::from(tempfile().unwrap()));
        let raw_recorded_compiler_args_fd = recorded_compiler_args_file.as_raw_fd();

        let mock_cancel_cb = MockICompilationTask::new();
        const EXPECTED_TIMEOUT_SECONDS: i32 = 32;
        mock_dex2oat_svc
            .expect_startVerifiedDex2Oat()
            .withf(move |args, record_fd, _, timeout_seconds| {
                let args_match = args.iter().zip(expected_args.iter()).all(|(actual, expected)| {
                    let actual_fds: Vec<i32> = actual.fds.iter().map(|fd| fd.as_raw_fd()).collect();
                    let expected_fds = &expected.fd_raw;
                    actual.formatString == expected.format_string && actual_fds == *expected_fds
                });
                args_match
                    && raw_recorded_compiler_args_fd == record_fd.as_raw_fd()
                    && timeout_seconds == &EXPECTED_TIMEOUT_SECONDS
            })
            .return_once(move |_, _, result_cbs, _| {
                // Invoke the callbacks to make sure they are the same callbacks provided in the
                // compilation context.
                let _ = result_cbs.onSuccess(&metrics);
                let _ = result_cbs.onFailure(&expected_failure_details);
                Ok(BnCompilationTask::new_binder(mock_cancel_cb, binder::BinderFeatures::default()))
            });
        let mut comp_ctx = CompilationContext {
            dex2oat_callback: BnDex2OatTaskCallback::new_binder(
                mock_dex_cb,
                binder::BinderFeatures::default(),
            ),
            cancellation_callback: None,
            service: BnIsolatedCompilationService::new_binder(
                mock_dex2oat_svc,
                binder::BinderFeatures::default(),
            ),
            args: dex2oat_args,
            recorded_compiler_args_fd: recorded_compiler_args_file,
            state: Arc::new(Mutex::new(CompilationState::Idle)),
        };
        assert_eq!(start_compilation(&mut comp_ctx, 32), FFIStatus_SUCCESS);
    }

    #[test]
    fn test_compile_start_dex2oat_no_args_failure() {
        let mut mock_dex_cb = MockIDex2OatTaskCallback::new();
        mock_dex_cb.expect_onSuccess().never();
        mock_dex_cb.expect_onFailure().never();
        let mut mock_dex2oat_svc = MockIIsolatedCompilationService::new();
        let recorded_compiler_args_file =
            ParcelFileDescriptor::new(OwnedFd::from(tempfile().unwrap()));

        mock_dex2oat_svc.expect_startVerifiedDex2Oat().never();
        let mut comp_ctx = CompilationContext {
            dex2oat_callback: BnDex2OatTaskCallback::new_binder(
                mock_dex_cb,
                binder::BinderFeatures::default(),
            ),
            cancellation_callback: None,
            service: BnIsolatedCompilationService::new_binder(
                mock_dex2oat_svc,
                binder::BinderFeatures::default(),
            ),
            args: Vec::new(),
            recorded_compiler_args_fd: recorded_compiler_args_file,
            state: Arc::new(Mutex::new(CompilationState::Idle)),
        };
        assert_eq!(start_compilation(&mut comp_ctx, 32), FFIStatus_CTX_MISSING_ARGS);
    }

    #[test]
    fn test_compile_double_start_dex2oat_fails() {
        let dex2oat_args: Vec<BnDex2OatArg> =
            vec![BnDex2OatArg { formatString: "FormatString".to_string(), fds: Vec::new() }];
        let mock_dex_cb = MockIDex2OatTaskCallback::new();
        let mut mock_dex2oat_svc = MockIIsolatedCompilationService::new();
        let recorded_compiler_args_file =
            ParcelFileDescriptor::new(OwnedFd::from(tempfile().unwrap()));
        let mock_cancel_cb = MockICompilationTask::new();
        mock_dex2oat_svc.expect_startVerifiedDex2Oat().times(1).return_once(move |_, _, _, _| {
            Ok(BnCompilationTask::new_binder(mock_cancel_cb, binder::BinderFeatures::default()))
        });
        let mut comp_ctx = CompilationContext {
            dex2oat_callback: BnDex2OatTaskCallback::new_binder(
                mock_dex_cb,
                binder::BinderFeatures::default(),
            ),
            cancellation_callback: None,
            service: BnIsolatedCompilationService::new_binder(
                mock_dex2oat_svc,
                binder::BinderFeatures::default(),
            ),
            args: dex2oat_args,
            recorded_compiler_args_fd: recorded_compiler_args_file,
            state: Arc::new(Mutex::new(CompilationState::Idle)),
        };

        assert_eq!(start_compilation(&mut comp_ctx, 32), FFIStatus_SUCCESS);

        assert_eq!(
            start_compilation(&mut comp_ctx, 32),
            FFIStatus_CTX_UNEXPECTED_COMPILATION_STATE
        );
    }
}
