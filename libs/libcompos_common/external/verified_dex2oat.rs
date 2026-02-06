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
    AVerifiedDex2Oat_FailureData as FFIFailureData,
    AVerifiedDex2Oat_FailureReason as FFIFailureReason,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_COMPILATION_SETUP_FAILED as FFIFailureReason_COMPILATION_SETUP_FAILED,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_DEX2OAT_FAILED as FFIFailureReason_DEX2OAT_FAILED,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_FAILED_TO_ENABLE_FSVERITY as FFIFailureReason_FAILED_TO_ENABLE_FSVERITY,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_FAILURE_UNKNOWN as FFIFailureReason_FAILURE_UNKNOWN,
    AVerifiedDex2Oat_FailureReason_AVERIFIED_DEX2OAT_TIMEOUT as FFIFailureReason_TIMEOUT,
    AVerifiedDex2Oat_Status as FFIStatus,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_BAD_ARGS as FFISTATUS_BAD_ARGS,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_BAD_ARGS_FORMAT_STRING_NOT_UTF8 as FFISTATUS_BAD_ARGS_FORMAT_STRING_NOT_UTF8,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS as FFISTATUS_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_CTX_MISSING_ARGS as FFISTATUS_CTX_MISSING_ARGS,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE as FFISTATUS_CTX_UNEXPECTED_COMPILATION_STATE,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_ERROR_CALLING_COMPOS as FFISTATUS_ERROR_CALLING_COMPOS,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE as FFISTATUS_COMPOS_SERVICE_UNAVAILABLE,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_ERROR_GENERAL as FFISTATUS_ERROR_GENERAL,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_ERROR_TIMED_OUT as FFISTATUS_ERROR_TIMED_OUT,
    AVerifiedDex2Oat_Status_AVERIFIED_DEX2OAT_SUCCESS as FFISTATUS_SUCCESS,
    AVerifiedDex2Oat_SuccessData as FFISuccessData,
    AVerifiedDex2Oat_onFailureCallback as FFIOnFailureCallback,
    AVerifiedDex2Oat_onSuccessCallback as FFIOnSuccessCallback,
};
use parking_lot::{Condvar, Mutex};
use std::{
    ffi::{c_char, c_int, CStr, CString},
    marker::{Send, Sync},
    mem::ManuallyDrop,
    os::{fd::BorrowedFd, raw::c_void},
    slice,
    sync::{Arc, Weak},
    thread,
    time::Duration,
};

const COMPILATION_STATE_MUTEX_TIMEOUT: Duration = Duration::from_millis(500);
const COMPILATION_STATE_MUTEX_TIMEOUT_LONG: Duration = Duration::from_millis(2500);

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
        _ => FFIFailureReason_FAILURE_UNKNOWN,
    }
}

#[derive(Clone)]
struct CallbackContext {
    cb_context: *mut c_void,
}

impl CallbackContext {
    fn get_inner(&self) -> *mut c_void {
        self.cb_context
    }
}

// SAFETY: Allow a void pointer to be sent across threads should be safe.
// The void pointer is provided by the caller and it is the caller's responsibility
// to make it thread safe.
unsafe impl Send for CallbackContext {}

// SAFETY: Allow a void pointer to be sent across threads should be safe.
// The void pointer is provided by the caller and it is the caller's responsibility
// to make it thread safe.
unsafe impl Sync for CallbackContext {}

struct Dex2OatCallback {
    on_success_c_cb: FFIOnSuccessCallback,
    success_user_data: CallbackContext,
    on_failure_c_cb: FFIOnFailureCallback,
    failure_user_data: CallbackContext,
    compilation_context: Weak<SharedCompCtxInner>,
}

impl binder::Interface for Dex2OatCallback {}
impl Dex2OatCallback {
    fn check_state_is_started(&self) -> binder::Result<()> {
        let upgrade_result = self.compilation_context.upgrade();
        if upgrade_result.is_none() {
            // Compilation context was destroyed so we don't care anymore.
            return Err(BnStatus::new_exception(
                BnExceptionCode::SERVICE_SPECIFIC,
                Some(c"Compilation state was already destroyed."),
            ));
        }
        let comp_mutex = upgrade_result.unwrap();
        let lock_result = comp_mutex.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
        if lock_result.is_none() {
            return Err(BnStatus::new_exception(
                BnExceptionCode::SERVICE_SPECIFIC,
                Some(c"Timed out while checking compilation state"),
            ));
        }
        let cur_state = &lock_result.unwrap().state;
        if *cur_state != CompilationState::Started {
            return Err(BnStatus::new_exception(
                BnExceptionCode::SERVICE_SPECIFIC,
                Some(c"Compilation was likely already canceled"),
            ));
        }
        Ok(())
    }

    fn set_state(&self, state: CompilationState) -> binder::Result<()> {
        let upgrade_result = self.compilation_context.upgrade();
        if upgrade_result.is_none() {
            // Compilation context was destroyed so we don't care anymore.
            return Err(BnStatus::new_exception(
                BnExceptionCode::SERVICE_SPECIFIC,
                Some(c"Compilation state was already destroyed."),
            ));
        }
        let comp_mutex = upgrade_result.unwrap();
        let lock_result = comp_mutex.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT_LONG);
        if lock_result.is_none() {
            return Err(BnStatus::new_exception(
                BnExceptionCode::SERVICE_SPECIFIC,
                Some(c"Timed out while setting compilation state"),
            ));
        }
        let mut comp_ctx = lock_result.unwrap();
        if comp_ctx.state == CompilationState::Started {
            comp_ctx.state = state;
        }
        Ok(())
    }
}
impl IDex2OatTaskCallback for Dex2OatCallback {
    fn onSuccess(&self, metrics: &Dex2OatMetrics) -> binder::Result<()> {
        if self.on_success_c_cb.is_none() {
            return Ok(());
        }
        self.check_state_is_started()?;
        let result_ctx = SuccessResultContext {
            wall_time_ms: metrics.wallclock_time_milliseconds.try_into().unwrap(),
            cpu_time_ms: metrics.cpu_time_milliseconds.try_into().unwrap(),
        };
        let success_data_ptr =
            (&result_ctx as *const SuccessResultContext) as *const FFISuccessData;
        let cb = self.on_success_c_cb.as_ref().unwrap();
        // SAFETY: on_success_c_cb and cb_ctx are all checked during
        // AVerifiedDex2Oat_createCompilationContext for non-nullness. The callers are
        // required to pass in a valid pointer to an appropriate callback function
        // during context creation.
        unsafe { (cb)(success_data_ptr, self.success_user_data.get_inner()) };
        self.set_state(CompilationState::Success)?;
        Ok(())
    }

    fn onFailure(&self, details: &Dex2OatFailureDetails) -> binder::Result<()> {
        if self.on_failure_c_cb.is_none() {
            return Ok(());
        }
        self.check_state_is_started()?;
        let reason = from_dex2oat_failure_reason(details.reason);
        let failure_data = FailureResultContext {
            reason,
            exit_code: details.exit_code,
            signal_code: if details.signal != 0 {
                Some(details.signal.try_into().unwrap())
            } else {
                None
            },
            cpu_time: details.cpu_time_milliseconds.try_into().unwrap(),
            wall_time: details.wallclock_time_milliseconds.try_into().unwrap(),
            message: CString::new(details.message.clone()).unwrap(),
        };
        let failure_data_ptr =
            (&failure_data as *const FailureResultContext) as *const FFIFailureData;

        let cb = self.on_failure_c_cb.as_ref().unwrap();
        // SAFETY: on_failure_c_cb and cb_ctx are all checked during
        // AVerifiedDex2Oat_createCompilationContext for non-nullness. The callers are
        // required to pass in a valid pointer to an appropriate callback function
        // during context creation.
        unsafe { (cb)(failure_data_ptr, self.failure_user_data.get_inner()) };
        self.set_state(CompilationState::Failed)?;
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
}

/// Holds the state and resources for a single dex2oat compilation.
///
/// This struct is created by `AVerifiedDex2Oat_CompilationContext_create` and released by
/// `AVerifiedDex2Oat_CompilationContext_release`.
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
    recorded_compiler_args_fd: Option<ParcelFileDescriptor>,
    state: CompilationState,
}

type SharedCompCtx = Arc<Mutex<CompilationContext>>;
type SharedCompCtxInner = Mutex<CompilationContext>;
impl CompilationContext {
    fn new_arc_mutex(service: Strong<dyn IIsolatedCompilationService>) -> SharedCompCtx {
        Arc::new(Mutex::new(CompilationContext {
            dex2oat_callback: None,
            cancellation_callback: None,
            service,
            args: Vec::new(),
            recorded_compiler_args_fd: None,
            state: CompilationState::Idle,
        }))
    }

    // Leak a SharedCompCtx.
    fn leak_mut(ctx: SharedCompCtx) -> *mut FFICompilationContext {
        Arc::into_raw(ctx) as *mut FFICompilationContext
    }
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
pub unsafe extern "C" fn AVerifiedDex2Oat_CompilationContext_create(
    out_ctx_ptr_ptr: *mut *mut FFICompilationContext,
    timeout_seconds: u64,
) -> i32 {
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
        return FFISTATUS_COMPOS_SERVICE_UNAVAILABLE;
    }
    let service = service_result.unwrap();
    let ctx = CompilationContext::new_arc_mutex(service);
    // SAFETY: `out_ctx` a non null pointer to a compilation context where `ctx` is null.
    // The rust code allocates a new context and attaches it to this compilation context.
    // It is now the responsibility of the API user to call release
    // on the compilation context to avoid leaks.
    unsafe {
        *out_ctx_ptr_ptr = CompilationContext::leak_mut(ctx);
    }
    FFISTATUS_SUCCESS
}

/// Add a single dex2oat argument to the compilation context.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
/// - `comp_ctx` must be a compilation context produced by
///   `AVerifiedDex2Oat_createCompilationContext`.
/// - `format_string` must be a UTF-8 null-terminated string.
/// - `fds` must point to a contiguous array of c_int, each entry must correspond to a valid, open,
///   file descriptor. The caller must relinquish ownership of these file descriptors after calling
///   this function.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_CompilationContext_addArg(
    comp_ctx: *mut FFICompilationContext,
    format_string: *const c_char,
    fds: *const c_int,
    fd_count: u32,
) -> FFIStatus {
    assert!(
        !comp_ctx.is_null(),
        "AVerifiedDex2Oat_CompilationContext_addArg called with a null compilation context"
    );
    assert!(
        !format_string.is_null(),
        "AVerifiedDex2Oat_CompilationContext_addArg called with a null "
    );
    assert!(!(fd_count > 0 && fds.is_null()), "AVerifiedDex2Oat_CompilationContext_addArg called with a non-zero fd_count but a null fds pointer");

    // SAFETY: The caller guarantees that `fds` points to a valid array of `c_int`
    // file descriptors with `fd_count` elements.
    let fds_slice =
        unsafe { slice::from_raw_parts(fds as *const c_int, fd_count.try_into().unwrap()) };

    let mut inner_fds: Vec<ParcelFileDescriptor> = Vec::new();
    for fd in fds_slice {
        // SAFETY: For F_GETFD any value of fd should be safe since an invalid file descriptor will
        // result in a `-1` return value.
        if unsafe { libc::fcntl(*fd, libc::F_GETFD) == -1 } {
            return FFISTATUS_BAD_ARGS;
        }
        // SAFETY: The caller guarantees that `fd` is a valid and open file descriptor.
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(*fd) };
        // Duplicate the file descriptor and turn the new duplicate fd into a parcel fd.
        inner_fds.push(ParcelFileDescriptor::new(borrowed_fd.try_clone_to_owned().unwrap()));
    }

    if comp_ctx.is_null() || format_string.is_null() {
        return FFISTATUS_BAD_ARGS;
    }

    // SAFETY: caller gives a guarantee that comp_ctx is a valid pointer to SharedCompCtxInner
    let ctx_mutex = unsafe { &*(comp_ctx as *const SharedCompCtxInner) };

    let lock_result = (*ctx_mutex).try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
    if lock_result.is_none() {
        return FFISTATUS_ERROR_TIMED_OUT;
    }
    let mut comp_ctx = lock_result.unwrap();
    if comp_ctx.state != CompilationState::Idle {
        return FFISTATUS_CTX_UNEXPECTED_COMPILATION_STATE;
    }

    // SAFETY: `format_string` is _Nonnullable and is specified to be a UTF-8 null terminated
    // string.
    let fmt_str = match unsafe { CStr::from_ptr(format_string) }.to_str() {
        Ok(s) => s,
        Err(_) => return FFISTATUS_BAD_ARGS_FORMAT_STRING_NOT_UTF8,
    };

    let placeholder_count = crate::wrappers::count_placeholders(fmt_str);
    if placeholder_count != fd_count {
        return FFISTATUS_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS;
    }

    if fd_count == 0 {
        comp_ctx.args.push(BnDex2OatArg { formatString: fmt_str.to_owned(), fds: inner_fds });
        return FFISTATUS_SUCCESS;
    }

    comp_ctx.args.push(BnDex2OatArg { formatString: fmt_str.to_owned(), fds: inner_fds });
    FFISTATUS_SUCCESS
}

struct SuccessResultContext {
    cpu_time_ms: u32,
    wall_time_ms: u32,
}

/// Extracts the wall time, in milliseconds, from an opaque result context.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///  - `success_data` must point to a `SuccessResultContext`
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_SuccessData_getWallClockTimeMs(
    success_data: *const FFISuccessData,
) -> u32 {
    assert!(
        !success_data.is_null(),
        "AVerifiedDex2Oat_SuccessData_getWallClockTimeMs called with a null success_data"
    );
    // SAFETY: Caller guarantees that success_data points to the success_data passed to
    // the on_success C callback. This is in turn guaranteed to be a SuccessResultContext.
    let success_result = unsafe { &(*(success_data as *const SuccessResultContext)) };
    success_result.wall_time_ms
}

/// Extracts the cpu time, in milliseconds, from an opaque result context.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///  - `success_data` must point to a `SuccessResultContext`` .
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_SuccessData_getCpuClockTimeMs(
    success_data: *const FFISuccessData,
) -> u32 {
    assert!(
        !success_data.is_null(),
        "AVerifiedDex2Oat_SuccessData_getCpuClockTimeMs called with a null success_data"
    );
    // SAFETY: Caller guarantees that success_data points to the success_data passed to
    // the on_success C callback. This is in turn guaranteed to be a SuccessResultContext.
    let success_result = unsafe { &*(success_data as *const SuccessResultContext) };
    success_result.cpu_time_ms
}

struct FailureResultContext {
    reason: FFIFailureReason,
    exit_code: i32,
    signal_code: Option<u32>,
    cpu_time: u32,
    wall_time: u32,
    message: CString,
}

/// Extracts the failure code from the opaque results context passed
/// into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `failure_data` must point to a FailureResultContext type.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureData_getReason(
    failure_data: *const FFIFailureData,
) -> FFIFailureReason {
    assert!(
        !failure_data.is_null(),
        "AVerifiedDex2Oat_FailureData_getReason called with a null failure_data"
    );
    // SAFETY: The caller guarantees that `failure_data` is the `failure_data`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_details = unsafe { &*(failure_data as *const FailureResultContext) };
    failure_details.reason
}

/// Extracts the failure exit code from the opaque results context passed
/// into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `failure_data` must point to a FailureResultContext type.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureData_getExitCode(
    failure_data: *const FFIFailureData,
) -> i32 {
    assert!(
        !failure_data.is_null(),
        "AVerifiedDex2Oat_FailureData_getExitCode called with a null failure_data"
    );
    // SAFETY: The caller guarantees that `failure_data` is the `failure_data`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_details = unsafe { &*(failure_data as *const FailureResultContext) };
    // If the reason is not DEX2OAT_FAILED then -1
    if failure_details.reason == FFIFailureReason_DEX2OAT_FAILED {
        return failure_details.exit_code;
    }
    -1
}

/// Extracts the failure exit code from the opaque results context passed
/// into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `failure_data` must point to a FailureResultContext type.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureData_getSignal(
    failure_data: *const FFIFailureData,
) -> u32 {
    assert!(
        !failure_data.is_null(),
        "AVerifiedDex2Oat_FailureData_getSignal called with a null failure_data"
    );
    // SAFETY: The caller guarantees that `failure_data` is the `failure_data`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_details = unsafe { &*(failure_data as *const FailureResultContext) };
    if let Some(signal_code) = failure_details.signal_code.as_ref() {
        return *signal_code;
    }
    0
}

/// Extracts the amount of CPU time spent on compilation before the failure occurred
/// from the opaque results context passed into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `failure_data` must point to a FailureResultContext type.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureData_getCpuClockTimeMs(
    failure_data: *const FFIFailureData,
) -> u32 {
    assert!(
        !failure_data.is_null(),
        "AVerifiedDex2Oat_FailureData_getCpuClockTimeMs called with a null failure_data"
    );
    // SAFETY: The caller guarantees that `failure_data` is the `failure_data`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_details = unsafe { &*(failure_data as *const FailureResultContext) };
    failure_details.cpu_time
}

/// Extracts the wallclock time spent on compilation before a failure occurred
/// from the opaque results context passed into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `failure_data` must point to a FailureResultContext type.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureData_getWallClockTimeMs(
    failure_data: *const FFIFailureData,
) -> u32 {
    assert!(
        !failure_data.is_null(),
        "AVerifiedDex2Oat_FailureData_getWallClockTimeMs called with a null failure_data"
    );
    // SAFETY: The caller guarantees that `failure_data` is the `failure_data`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_details = unsafe { &*(failure_data as *const FailureResultContext) };
    failure_details.wall_time
}

/// Extracts the failure code message from the opaque results context passed
/// into a `AVerifiedDex2Oat_OnFailureCallback`.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
///   - `failure_data` must point to a FailureResultContext type.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_FailureData_getMessage(
    failure_data: *const FFIFailureData,
) -> *const c_char {
    assert!(
        !failure_data.is_null(),
        "AVerifiedDex2Oat_FailureData_getMessage called with a null failure_data"
    );
    // SAFETY: The caller guarantees that `failure_data` is the `failure_data`
    // passed into `AVerifiedDex2Oat_OnFailureCallback`, which is guaranteed to be valid.
    let failure_result = unsafe { &*(failure_data as *const FailureResultContext) };
    failure_result.message.as_ptr()
}

/// Converts a `AVerifiedDex2Oat_FailureReason` enum to its string representation.
///
/// Refer to the public C API header for full documentation.
#[no_mangle]
pub extern "C" fn AVerifiedDex2Oat_FailureReason_toString(
    reason: FFIFailureReason,
) -> *const c_char {
    let reason_str: &'static CStr;
    if reason == FFIFailureReason_COMPILATION_SETUP_FAILED {
        reason_str = c"AVERIFIED_DEX2OAT_COMPILATION_SETUP_FAILED";
    } else if reason == FFIFailureReason_DEX2OAT_FAILED {
        reason_str = c"AVERIFIED_DEX2OAT_DEX2OAT_FAILED";
    } else if reason == FFIFailureReason_FAILED_TO_ENABLE_FSVERITY {
        reason_str = c"AVERIFIED_DEX2OAT_FAILED_TO_ENABLE_FSVERITY";
    } else if reason == FFIFailureReason_TIMEOUT {
        reason_str = c"AVERIFIED_DEX2OAT_TIMEOUT";
    } else {
        reason_str = c"INVALID_FAILURE_REASON";
    }
    reason_str.as_ptr()
}

/// Converts a `AVerifiedDex2Oat_Status` enum to its string representation.
///
/// Refer to the public C API header for full documentation.
#[no_mangle]
pub extern "C" fn AVerifiedDex2Oat_Status_toString(status: FFIStatus) -> *const c_char {
    let status_str: &'static CStr = match status {
        FFISTATUS_SUCCESS => c"AVERIFIED_DEX2OAT_SUCCESS",
        FFISTATUS_ERROR_GENERAL => c"AVERIFIED_DEX2OAT_ERROR_GENERAL",
        FFISTATUS_ERROR_TIMED_OUT => c"AVERIFIED_DEX2OAT_ERROR_TIMED_OUT",
        FFISTATUS_COMPOS_SERVICE_UNAVAILABLE => {
            c"AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE"
        }
        FFISTATUS_ERROR_CALLING_COMPOS => c"AVERIFIED_DEX2OAT_ERROR_CALLING_COMPOS",
        FFISTATUS_BAD_ARGS => c"AVERIFIED_DEX2OAT_BAD_ARGS",
        FFISTATUS_BAD_ARGS_FORMAT_STRING_NOT_UTF8 => {
            c"AVERIFIED_DEX2OAT_BAD_ARGS_FORMAT_STRING_NOT_UTF8"
        }
        FFISTATUS_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS => {
            c"AVERIFIED_DEX2OAT_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS"
        }
        FFISTATUS_CTX_UNEXPECTED_COMPILATION_STATE => {
            c"AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE"
        }
        FFISTATUS_CTX_MISSING_ARGS => c"AVERIFIED_DEX2OAT_CTX_MISSING_ARGS",
        _ => c"INVALID_STATUS",
    };
    status_str.as_ptr()
}

/// Destroys a compilation context.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
/// The caller must guarantee:
/// - `comp_ctx` points to a pointer that points to a `CompilationContext` or is null.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_CompilationContext_destroy(
    comp_ctx: *const FFICompilationContext,
) {
    if comp_ctx.is_null() {
        return;
    }
    // SAFETY: Caller guarantees that comp_ctx points to a FFICompilationContext created by
    // `AVerifiedDex2Oat_CompilationContext_create`. This decrements the ref count which
    // will result it in being dropped.
    let _ = unsafe { Arc::from_raw(comp_ctx as *const SharedCompCtxInner) };
}

/// Starts a dex2oat compilation within a VM.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
/// The caller must guarantee:
/// - `comp_ctx` was created using `AVerifiedDex2Oat_createCompilationContext`
/// - No other process is concurrently accessing `compilation_ctx` for the duration of this function
/// - `recorded_compiler_args_fd` this file descriptor must be valid.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_start(
    comp_ctx: *mut FFICompilationContext,
    on_success_c_cb: FFIOnSuccessCallback,
    success_user_data: *mut c_void,
    on_failure_c_cb: FFIOnFailureCallback,
    failure_user_data: *mut c_void,
    recorded_compiler_args_fd: c_int,
    timeout_seconds: u32,
) -> FFIStatus {
    assert!(!comp_ctx.is_null(), "AVerifiedDex2Oat_start called with a null compilation context");
    assert!(
        on_success_c_cb.is_some(),
        "AVerifiedDex2Oat_start called with a null success callback function pointer"
    );
    assert!(
        on_failure_c_cb.is_some(),
        "AVerifiedDex2Oat_start called with a null failure callback function pointer"
    );
    if timeout_seconds == 0 {
        return FFISTATUS_BAD_ARGS;
    }
    let ctx_mutex = ManuallyDrop::new(
        // SAFETY: The caller guarantees that `comp_ctx` is a valid pointer to a
        // `CompilationContext`. We are dereferencing it to access the context.
        // We intentionally unpack this back into an Arc since we need to an Arc to create a
        // downgraded reference for Dex2OatCallback.compilation_context. To prevent the
        // refcount from decrementing we use ManuallyDrop to disable Arc's destructor.
        unsafe { Arc::from_raw(comp_ctx as *const SharedCompCtxInner) },
    );
    let ctx_lock_result = ctx_mutex.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
    if ctx_lock_result.is_none() {
        return FFISTATUS_ERROR_TIMED_OUT;
    }
    let mut ctx = ctx_lock_result.unwrap();
    if ctx.state != CompilationState::Idle {
        return FFISTATUS_CTX_UNEXPECTED_COMPILATION_STATE;
    }
    if ctx.args.is_empty() {
        return FFISTATUS_CTX_MISSING_ARGS;
    }
    let callback = Dex2OatCallback {
        on_success_c_cb,
        success_user_data: CallbackContext { cb_context: success_user_data },
        on_failure_c_cb,
        failure_user_data: CallbackContext { cb_context: failure_user_data },
        compilation_context: Arc::downgrade(&*ctx_mutex),
    };
    let dex2oat_callback =
        BnDex2OatTaskCallback::new_binder(callback, binder::BinderFeatures::default());

    let svc = ctx.service.clone();

    // SAFETY: `recorded_compiler_args_fd` is provided by the C caller, it is the caller's
    // responsibility to ensure that the file descriptor is valid.
    let borrowed_args_fd = unsafe { BorrowedFd::borrow_raw(recorded_compiler_args_fd) };
    // We dupe this valid file descriptor so the caller can continue using the original file
    // descriptor.
    let compiler_arg_parcel_fd =
        ParcelFileDescriptor::new(borrowed_args_fd.try_clone_to_owned().unwrap());
    ctx.dex2oat_callback = Some(dex2oat_callback);
    match svc.startVerifiedDex2Oat(
        &ctx.args,
        &compiler_arg_parcel_fd,
        ctx.dex2oat_callback.as_ref().unwrap(),
        timeout_seconds.try_into().unwrap(),
    ) {
        Err(_) => {
            return FFISTATUS_ERROR_CALLING_COMPOS;
        }
        Ok(cb) => {
            ctx.cancellation_callback = Some(cb);
            ctx.state = CompilationState::Started;
        }
    }
    ctx.recorded_compiler_args_fd = Some(compiler_arg_parcel_fd);
    FFISTATUS_SUCCESS
}

/// Cancels an ongoing dex2oat compilation.
///
/// Refer to the public C API header for full documentation.
///
/// # Safety
/// The caller must guarantee:
/// - `comp_ctx` is a valid context created by `AVerifiedDex2Oat_createCompilationContext`
/// - No other process is concurrently accessing `compilation_ctx` for the duration of this function
///   call.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_cancel(
    comp_ctx: *mut FFICompilationContext,
) -> FFIStatus {
    assert!(!comp_ctx.is_null(), "AVerifiedDex2Oat_cancel called with a null compilation context");
    // SAFETY: The caller guarantees that `ctx` is a valid pointer to a `CompilationContext`.
    // We are dereferencing it to access the context.
    let comp_mutex = unsafe { &*(comp_ctx as *const SharedCompCtxInner) };
    let guard_result = comp_mutex.try_lock_for(COMPILATION_STATE_MUTEX_TIMEOUT);
    if guard_result.is_none() {
        return FFISTATUS_ERROR_TIMED_OUT;
    }
    let mut ctx = guard_result.unwrap();
    match ctx.state {
        CompilationState::Started => {
            if let Some(cancellation_task) = &ctx.cancellation_callback {
                if cancellation_task.cancel().is_err() {
                    return FFISTATUS_ERROR_CALLING_COMPOS;
                }
                ctx.state = CompilationState::Canceled;
                return FFISTATUS_SUCCESS;
            }
            FFISTATUS_ERROR_GENERAL
        }
        _ => FFISTATUS_CTX_UNEXPECTED_COMPILATION_STATE,
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
    fn build_empty_comp_ctx() -> SharedCompCtx {
        Arc::new(Mutex::new(CompilationContext {
            dex2oat_callback: Some(BnDex2OatTaskCallback::new_binder(
                MockIDex2OatTaskCallback::new(),
                binder::BinderFeatures::default(),
            )),
            cancellation_callback: None,
            args: Vec::new(),
            recorded_compiler_args_fd: Some(ParcelFileDescriptor::new(OwnedFd::from(
                tempfile().unwrap(),
            ))),
            service: BnIsolatedCompilationService::new_binder(
                MockIIsolatedCompilationService::new(),
                binder::BinderFeatures::default(),
            ),
            state: CompilationState::Idle,
        }))
    }

    // For a vector of files return a vector containing their file descriptors.
    fn files_as_fds(files: &[File]) -> Vec<i32> {
        files.iter().map(|file| file.as_raw_fd()).collect()
    }

    fn get_temp_file_vec(count: usize) -> Vec<File> {
        std::iter::repeat_with(|| tempfile().unwrap()).take(count).collect()
    }

    fn add_arg_to_compilation_context(
        ctx: &SharedCompCtx,
        format_string: &CString,
        fds: &[i32],
    ) -> i32 {
        let compilation_ctx = Arc::as_ptr(ctx) as *mut FFICompilationContext;
        // SAFETY: `ctx_ptr` is defined above as a valid pointer to a CompilationContext
        // variable. `format_string`, `fds` are both guaranteed to be a UTF-8
        // encoded C-strings and vectors of i32s.
        unsafe {
            AVerifiedDex2Oat_CompilationContext_addArg(
                compilation_ctx,
                format_string.as_ptr(),
                fds.as_ptr(),
                fds.len().try_into().unwrap(),
            )
        }
    }

    fn start_compilation(
        ctx: &SharedCompCtx,
        on_success_c_cb: FFIOnSuccessCallback,
        on_failure_c_cb: FFIOnFailureCallback,
        user_data: &mut MockResultCallBackVerifierInterface,
        compiler_args_file: &File,
        timeout_seconds: u32,
    ) -> i32 {
        let user_data_ptr = (user_data as *mut MockResultCallBackVerifierInterface) as *mut c_void;
        let raw_ctx = Arc::as_ptr(ctx) as *mut FFICompilationContext;
        // SAFETY: all the parameters meet the safety requirements. raw_ctx is derived from a valid
        // SharedCompCtx the callbacks are both valid, etc...
        unsafe {
            AVerifiedDex2Oat_start(
                raw_ctx,
                on_success_c_cb,
                user_data_ptr,
                on_failure_c_cb,
                user_data_ptr,
                compiler_args_file.as_raw_fd(),
                timeout_seconds,
            )
        }
    }

    fn create_compilation_context(
        out_ctx_ptr_ptr: *mut *mut FFICompilationContext,
        timeout_seconds: u64,
    ) -> i32 {
        // SAFETY: out_ctx_ptr_ptr is a pointer to a valid pointer (which points at nothing).
        //  - on_success_fn_ptr is a function of type OnSuccessCallback with static lifetime
        //  - on_failure_fn_ptr is a function of type OnFailureCallback with static lifetime.
        //  - cb_ctx is a valid pointer created by Box::into_raw with a lifetime of this test.
        //  - recorded_args_fd - is a raw_fd of a File created by tempfile(). the lifetime of the
        //    File is the lifetime of this test.
        unsafe { AVerifiedDex2Oat_CompilationContext_create(out_ctx_ptr_ptr, timeout_seconds) }
    }

    fn build_comp_ctx(
        mock_svc: MockIIsolatedCompilationService,
        args: Vec<BnDex2OatArg>,
    ) -> SharedCompCtx {
        Arc::new(Mutex::new(CompilationContext {
            dex2oat_callback: None,
            cancellation_callback: None,
            service: BnIsolatedCompilationService::new_binder(
                mock_svc,
                binder::BinderFeatures::default(),
            ),
            args,
            recorded_compiler_args_fd: None,
            state: CompilationState::Idle,
        }))
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

    /// A mockable trait used to verify that the C-style callbacks (`on_success` and `on_failure`)
    /// are invoked with the correct arguments from within the Rust FFI layer.
    #[mockall::automock]
    trait ResultCallBackVerifierInterface {
        fn on_success(&self, cpu_time_ms: u32, wall_time_ms: u32);
        fn on_failure(
            &self,
            failure_reason: FFIFailureReason,
            exit_code: i32,
            signal: u32,
            cpu_time_ms: u32,
            wall_time_ms: u32,
            message: &CStr,
        );
    }

    /// C-style function pointer that acts as a bridge to the `on_success` method of the
    /// `MockResultCallBackVerifierInterface`.
    ///
    /// # Safety
    /// - `success_user_data` must be valid pointer to `MockResultCallBackVerifierInterface`.
    /// - `success_data` must be a valid pointer to a `SuccessData`
    unsafe extern "C" fn on_success_fn_ptr(
        success_data: *const FFISuccessData,
        success_user_data: *mut c_void,
    ) {
        assert!(!success_user_data.is_null());
        assert!(!success_data.is_null());
        // SAFETY: Unit test code, ctx is guaranteed to be the correct type.
        let mock = unsafe { &*(success_user_data as *const MockResultCallBackVerifierInterface) };
        // SAFETY: cpu_time_ms and wall_time_ms are both appropriately typed
        // result_ctx is guaranteed to be backed by a SuccessResultContext.
        unsafe {
            let cpu_time_ms = AVerifiedDex2Oat_SuccessData_getCpuClockTimeMs(success_data);
            let wall_time_ms = AVerifiedDex2Oat_SuccessData_getWallClockTimeMs(success_data);
            mock.on_success(cpu_time_ms, wall_time_ms);
        }
    }

    /// C-style function pointer that acts as a bridge to the `on_failure` method of the
    /// `MockResultCallBackVerifierInterface`.
    ///
    /// # Safety
    /// - `ctx` must be a valid pointer to `MockResultCallBackVerifierInterface`
    /// - `message` must be a valid pointer to a null-terminated C string.
    unsafe extern "C" fn on_failure_fn_ptr(
        failure_data: *const FFIFailureData,
        failure_user_data: *mut c_void,
    ) {
        assert!(!failure_user_data.is_null());
        assert!(!failure_data.is_null());
        // SAFETY: Unit test code, cb_ctx is guaranteed to be the correct type.
        let mock = unsafe { &*(failure_user_data as *const MockResultCallBackVerifierInterface) };
        // SAFETY: result_ctx is guaranteed to be backed by a FailureResultContext
        // failure_code and c_char_ptr are valid, see previous lines of code.
        unsafe {
            let failure_reason = AVerifiedDex2Oat_FailureData_getReason(failure_data);
            let exit_code = AVerifiedDex2Oat_FailureData_getExitCode(failure_data);
            let signal = AVerifiedDex2Oat_FailureData_getSignal(failure_data);
            let cpu_time = AVerifiedDex2Oat_FailureData_getCpuClockTimeMs(failure_data);
            let wall_time = AVerifiedDex2Oat_FailureData_getWallClockTimeMs(failure_data);
            let c_char_ptr = AVerifiedDex2Oat_FailureData_getMessage(failure_data);
            let message: &CStr = CStr::from_ptr(c_char_ptr);
            mock.on_failure(failure_reason, exit_code, signal, cpu_time, wall_time, message);
        };
    }

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

        let mut opaque_comp_ctx: *mut FFICompilationContext = std::ptr::null_mut();
        let result = create_compilation_context(&mut opaque_comp_ctx, 1);

        assert_eq!(result, FFISTATUS_SUCCESS);

        // Clean up the created context.
        // SAFETY: `opaque_comp_ctx` was initialized by
        // `AVerifiedDex2Oat_createCompilationContext`, satisfying the safety
        // requirements of `AVerifiedDex2Oat_destroyCompilationContext`.
        unsafe { AVerifiedDex2Oat_CompilationContext_destroy(opaque_comp_ctx) };
    }

    #[test]
    fn test_add_args_success() {
        let comp_mutex = build_empty_comp_ctx();

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
                add_arg_to_compilation_context(&comp_mutex, format_str, &fds),
                FFISTATUS_SUCCESS
            );
        }
        let comp_ctx = comp_mutex.lock();
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
        let comp_mutex = build_empty_comp_ctx();
        {
            let mut comp_ctx = comp_mutex.lock();
            comp_ctx.state = CompilationState::Started;
            let mock_cancel_cb = BnCompilationTask::new_binder(
                MockICompilationTask::new(),
                binder::BinderFeatures::default(),
            );
            comp_ctx.cancellation_callback = Some(mock_cancel_cb);
        }

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
                add_arg_to_compilation_context(&comp_mutex, format_str, &fds),
                FFISTATUS_CTX_UNEXPECTED_COMPILATION_STATE
            );
        }
    }

    #[test]
    fn test_add_args_when_placeholder_count_ne_fd_count_failure() {
        let comp_ctx = build_empty_comp_ctx();
        let format_str = CString::new("ThreePlaceholders!!!").unwrap();
        let file = tempfile().unwrap();
        let fds = [file.as_raw_fd()];
        assert_eq!(
            add_arg_to_compilation_context(&comp_ctx, &format_str, &fds),
            FFISTATUS_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS
        );
    }

    #[test]
    fn test_add_args_with_bad_fds_failure() {
        let comp_ctx = build_empty_comp_ctx();
        let format_str = CString::new("FormatString!").unwrap();
        let fds = [tempfile().unwrap().as_raw_fd()];
        assert_eq!(
            add_arg_to_compilation_context(&comp_ctx, &format_str, &fds),
            FFISTATUS_BAD_ARGS
        );
    }

    #[test]
    fn test_compile_start_dex2oat_success() {
        let build_arg = |fd_count: usize| -> BnDex2OatArg {
            BnDex2OatArg {
                formatString: format!("FormatString{}", "!".repeat(fd_count)),
                fds: (0..fd_count)
                    .map(|_| ParcelFileDescriptor::new::<OwnedFd>(tempfile().unwrap().into()))
                    .collect(),
            }
        };
        struct TestArgs {
            format_string: String,
            fd_raw: Vec<i32>,
        }

        let dex2oat_args = vec![build_arg(3), build_arg(2), build_arg(10)];
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
            signal: 23,
            cpu_time_milliseconds: 456,
            wallclock_time_milliseconds: 654,
            message: "failure_message".to_string(),
        };
        let mut mock_cb_verifier = MockResultCallBackVerifierInterface::new();

        let expected_cpu_time = metrics.cpu_time_milliseconds;
        let expected_wall_time = metrics.wallclock_time_milliseconds;
        mock_cb_verifier
            .expect_on_success()
            .with(
                eq::<u32>(expected_cpu_time.try_into().unwrap()),
                eq::<u32>(expected_wall_time.try_into().unwrap()),
            )
            .return_once(|_, _| ());
        mock_cb_verifier
            .expect_on_failure()
            .with(
                eq(from_dex2oat_failure_reason(expected_failure_details.reason)),
                eq(expected_failure_details.exit_code),
                eq::<u32>(expected_failure_details.signal.try_into().unwrap()),
                eq::<u32>(expected_failure_details.cpu_time_milliseconds.try_into().unwrap()),
                eq::<u32>(expected_failure_details.wallclock_time_milliseconds.try_into().unwrap()),
                any(),
            )
            .return_once(|_, _, _, _, _, _| ());
        let mut mock_dex2oat_svc = MockIIsolatedCompilationService::new();
        let recorded_compiler_args_file = tempfile().unwrap();
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
                    && fds_are_equivalent(raw_recorded_compiler_args_fd, record_fd.as_raw_fd())
                    && timeout_seconds == &EXPECTED_TIMEOUT_SECONDS
            })
            .return_once(move |_, _, result_cbs, _| {
                // Invoke the callbacks to make sure they are the same callbacks provided in the
                // compilation context.
                let _ = result_cbs.onSuccess(&metrics);
                let _ = result_cbs.onFailure(&expected_failure_details);
                Ok(BnCompilationTask::new_binder(mock_cancel_cb, binder::BinderFeatures::default()))
            });
        let comp_ctx = build_comp_ctx(mock_dex2oat_svc, dex2oat_args);

        assert_eq!(
            start_compilation(
                &comp_ctx,
                Some(on_success_fn_ptr),
                Some(on_failure_fn_ptr),
                &mut mock_cb_verifier,
                &recorded_compiler_args_file,
                32
            ),
            FFISTATUS_SUCCESS
        );
    }

    #[test]
    fn test_compile_start_dex2oat_no_args_failure() {
        let mut mock_dex2oat_svc = MockIIsolatedCompilationService::new();
        let recorded_compiler_args_file = tempfile().unwrap();

        mock_dex2oat_svc.expect_startVerifiedDex2Oat().never();
        let comp_ctx = build_comp_ctx(mock_dex2oat_svc, Vec::new());

        let mut mock_cb_verifier = MockResultCallBackVerifierInterface::new();
        mock_cb_verifier.expect_on_success().never();
        mock_cb_verifier.expect_on_failure().never();
        assert_eq!(
            start_compilation(
                &comp_ctx,
                Some(on_success_fn_ptr),
                Some(on_failure_fn_ptr),
                &mut mock_cb_verifier,
                &recorded_compiler_args_file,
                32
            ),
            FFISTATUS_CTX_MISSING_ARGS
        );
    }

    #[test]
    fn test_compile_double_start_dex2oat_fails() {
        let dex2oat_args: Vec<BnDex2OatArg> =
            vec![BnDex2OatArg { formatString: "FormatString".to_string(), fds: Vec::new() }];
        let mut mock_dex2oat_svc = MockIIsolatedCompilationService::new();
        let recorded_compiler_args_file = tempfile().unwrap();
        let mock_cancel_cb = MockICompilationTask::new();
        mock_dex2oat_svc.expect_startVerifiedDex2Oat().times(1).return_once(move |_, _, _, _| {
            Ok(BnCompilationTask::new_binder(mock_cancel_cb, binder::BinderFeatures::default()))
        });
        let comp_ctx = build_comp_ctx(mock_dex2oat_svc, dex2oat_args);

        let mut mock_cb_verifier = MockResultCallBackVerifierInterface::new();
        mock_cb_verifier.expect_on_success().never();
        mock_cb_verifier.expect_on_failure().never();

        assert_eq!(
            start_compilation(
                &comp_ctx,
                Some(on_success_fn_ptr),
                Some(on_failure_fn_ptr),
                &mut mock_cb_verifier,
                &recorded_compiler_args_file,
                32
            ),
            FFISTATUS_SUCCESS
        );
        assert_eq!(
            start_compilation(
                &comp_ctx,
                Some(on_success_fn_ptr),
                Some(on_failure_fn_ptr),
                &mut mock_cb_verifier,
                &recorded_compiler_args_file,
                32
            ),
            FFISTATUS_CTX_UNEXPECTED_COMPILATION_STATE
        );
    }
}
