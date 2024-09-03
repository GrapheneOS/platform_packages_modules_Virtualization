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

//! This module handles the interaction with virtual machine payload service.

use android_system_virtualization_payload::aidl::android::system::virtualization::payload:: IVmPayloadService::{
    IVmPayloadService, ENCRYPTEDSTORE_MOUNTPOINT, VM_APK_CONTENTS_PATH,
    VM_PAYLOAD_SERVICE_SOCKET_NAME, AttestationResult::AttestationResult,
};
use anyhow::{bail, ensure, Context, Result};
use binder::{
    unstable_api::{new_spibinder, AIBinder},
    Strong, ExceptionCode,
};
use log::{error, info, LevelFilter};
use rpcbinder::{RpcServer, RpcSession};
use openssl::{ec::EcKey, sha::sha256, ecdsa::EcdsaSig};
use std::convert::Infallible;
use std::ffi::{CString, CStr};
use std::fmt::Debug;
use std::os::raw::{c_char, c_void};
use std::path::Path;
use std::ptr::{self, NonNull};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    LazyLock,
    Mutex,
};
use vm_payload_status_bindgen::AVmAttestationStatus;

/// Maximum size of an ECDSA signature for EC P-256 key is 72 bytes.
const MAX_ECDSA_P256_SIGNATURE_SIZE: usize = 72;

static VM_APK_CONTENTS_PATH_C: LazyLock<CString> =
    LazyLock::new(|| CString::new(VM_APK_CONTENTS_PATH).expect("CString::new failed"));
static PAYLOAD_CONNECTION: Mutex<Option<Strong<dyn IVmPayloadService>>> = Mutex::new(None);
static VM_ENCRYPTED_STORAGE_PATH_C: LazyLock<CString> =
    LazyLock::new(|| CString::new(ENCRYPTEDSTORE_MOUNTPOINT).expect("CString::new failed"));

static ALREADY_NOTIFIED: AtomicBool = AtomicBool::new(false);

/// Return a connection to the payload service in Microdroid Manager. Uses the existing connection
/// if there is one, otherwise attempts to create a new one.
fn get_vm_payload_service() -> Result<Strong<dyn IVmPayloadService>> {
    let mut connection = PAYLOAD_CONNECTION.lock().unwrap();
    if let Some(strong) = &*connection {
        Ok(strong.clone())
    } else {
        let new_connection: Strong<dyn IVmPayloadService> = RpcSession::new()
            .setup_unix_domain_client(VM_PAYLOAD_SERVICE_SOCKET_NAME)
            .context(format!("Failed to connect to service: {}", VM_PAYLOAD_SERVICE_SOCKET_NAME))?;
        *connection = Some(new_connection.clone());
        Ok(new_connection)
    }
}

/// Make sure our logging goes to logcat. It is harmless to call this more than once.
fn initialize_logging() {
    android_logger::init_once(
        android_logger::Config::default().with_tag("vm_payload").with_max_level(LevelFilter::Info),
    );
}

/// In many cases clients can't do anything useful if API calls fail, and the failure
/// generally indicates that the VM is exiting or otherwise doomed. So rather than
/// returning a non-actionable error indication we just log the problem and abort
/// the process.
fn unwrap_or_abort<T, E: Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|e| {
        let msg = format!("{:?}", e);
        error!("{msg}");
        panic!("{msg}")
    })
}

/// Notifies the host that the payload is ready.
/// Panics on failure.
#[no_mangle]
pub extern "C" fn AVmPayload_notifyPayloadReady() {
    initialize_logging();

    if !ALREADY_NOTIFIED.swap(true, Ordering::Relaxed) {
        unwrap_or_abort(try_notify_payload_ready());

        info!("Notified host payload ready successfully");
    }
}

/// Notifies the host that the payload is ready.
/// Returns a `Result` containing error information if failed.
fn try_notify_payload_ready() -> Result<()> {
    get_vm_payload_service()?.notifyPayloadReady().context("Cannot notify payload ready")
}

/// Runs a binder RPC server, serving the supplied binder service implementation on the given vsock
/// port.
///
/// If and when the server is ready for connections (it is listening on the port), `on_ready` is
/// called to allow appropriate action to be taken - e.g. to notify clients that they may now
/// attempt to connect.
///
/// The current thread joins the binder thread pool to handle incoming messages.
/// This function never returns.
///
/// Panics on error (including unexpected server exit).
///
/// # Safety
///
/// If present, the `on_ready` callback must be a valid function pointer, which will be called at
/// most once, while this function is executing, with the `param` parameter.
#[no_mangle]
pub unsafe extern "C" fn AVmPayload_runVsockRpcServer(
    service: *mut AIBinder,
    port: u32,
    on_ready: Option<unsafe extern "C" fn(param: *mut c_void)>,
    param: *mut c_void,
) -> Infallible {
    initialize_logging();

    // SAFETY: try_run_vsock_server has the same requirements as this function
    unwrap_or_abort(unsafe { try_run_vsock_server(service, port, on_ready, param) })
}

/// # Safety: Same as `AVmPayload_runVsockRpcServer`.
unsafe fn try_run_vsock_server(
    service: *mut AIBinder,
    port: u32,
    on_ready: Option<unsafe extern "C" fn(param: *mut c_void)>,
    param: *mut c_void,
) -> Result<Infallible> {
    // SAFETY: AIBinder returned has correct reference count, and the ownership can
    // safely be taken by new_spibinder.
    let service = unsafe { new_spibinder(service) };
    if let Some(service) = service {
        match RpcServer::new_vsock(service, libc::VMADDR_CID_HOST, port) {
            Ok(server) => {
                if let Some(on_ready) = on_ready {
                    // SAFETY: We're calling the callback with the parameter specified within the
                    // allowed lifetime.
                    unsafe { on_ready(param) };
                }
                server.join();
                bail!("RpcServer unexpectedly terminated");
            }
            Err(err) => {
                bail!("Failed to start RpcServer: {:?}", err);
            }
        }
    } else {
        bail!("Failed to convert the given service from AIBinder to SpIBinder.");
    }
}

/// Get a secret that is uniquely bound to this VM instance.
/// Panics on failure.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `identifier` must be [valid] for reads of `identifier_size` bytes.
/// * `secret` must be [valid] for writes of `size` bytes.
///
/// [valid]: ptr#safety
#[no_mangle]
pub unsafe extern "C" fn AVmPayload_getVmInstanceSecret(
    identifier: *const u8,
    identifier_size: usize,
    secret: *mut u8,
    size: usize,
) {
    initialize_logging();

    // SAFETY: See the requirements on `identifier` above.
    let identifier = unsafe { std::slice::from_raw_parts(identifier, identifier_size) };
    let vm_secret = unwrap_or_abort(try_get_vm_instance_secret(identifier, size));

    // SAFETY: See the requirements on `secret` above; `vm_secret` is known to have length `size`,
    // and cannot overlap `secret` because we just allocated it.
    unsafe {
        ptr::copy_nonoverlapping(vm_secret.as_ptr(), secret, size);
    }
}

fn try_get_vm_instance_secret(identifier: &[u8], size: usize) -> Result<Vec<u8>> {
    let vm_secret = get_vm_payload_service()?
        .getVmInstanceSecret(identifier, i32::try_from(size)?)
        .context("Cannot get VM instance secret")?;
    ensure!(
        vm_secret.len() == size,
        "Returned secret has {} bytes, expected {}",
        vm_secret.len(),
        size
    );
    Ok(vm_secret)
}

/// Get the VM's attestation chain.
/// Panics on failure.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `data` must be [valid] for writes of `size` bytes, if size > 0.
///
/// [valid]: ptr#safety
#[no_mangle]
pub unsafe extern "C" fn AVmPayload_getDiceAttestationChain(data: *mut u8, size: usize) -> usize {
    initialize_logging();

    let chain = unwrap_or_abort(try_get_dice_attestation_chain());
    if size != 0 {
        // SAFETY: See the requirements on `data` above. The number of bytes copied doesn't exceed
        // the length of either buffer, and `chain` cannot overlap `data` because we just allocated
        // it. We allow data to be null, which is never valid, but only if size == 0 which is
        // checked above.
        unsafe { ptr::copy_nonoverlapping(chain.as_ptr(), data, std::cmp::min(chain.len(), size)) };
    }
    chain.len()
}

fn try_get_dice_attestation_chain() -> Result<Vec<u8>> {
    get_vm_payload_service()?.getDiceAttestationChain().context("Cannot get attestation chain")
}

/// Get the VM's attestation CDI.
/// Panics on failure.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `data` must be [valid] for writes of `size` bytes, if size > 0.
///
/// [valid]: ptr#safety
#[no_mangle]
pub unsafe extern "C" fn AVmPayload_getDiceAttestationCdi(data: *mut u8, size: usize) -> usize {
    initialize_logging();

    let cdi = unwrap_or_abort(try_get_dice_attestation_cdi());
    if size != 0 {
        // SAFETY: See the requirements on `data` above. The number of bytes copied doesn't exceed
        // the length of either buffer, and `cdi` cannot overlap `data` because we just allocated
        // it. We allow data to be null, which is never valid, but only if size == 0 which is
        // checked above.
        unsafe { ptr::copy_nonoverlapping(cdi.as_ptr(), data, std::cmp::min(cdi.len(), size)) };
    }
    cdi.len()
}

fn try_get_dice_attestation_cdi() -> Result<Vec<u8>> {
    get_vm_payload_service()?.getDiceAttestationCdi().context("Cannot get attestation CDI")
}

/// Requests the remote attestation of the client VM.
///
/// The challenge will be included in the certificate chain in the attestation result,
/// serving as proof of the freshness of the result.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `challenge` must be [valid] for reads of `challenge_size` bytes.
///
/// [valid]: ptr#safety
#[no_mangle]
pub unsafe extern "C" fn AVmPayload_requestAttestation(
    challenge: *const u8,
    challenge_size: usize,
    res: &mut *mut AttestationResult,
) -> AVmAttestationStatus {
    // SAFETY: The caller guarantees that `challenge` is valid for reads and `res` is valid
    // for writes.
    unsafe {
        request_attestation(
            challenge,
            challenge_size,
            false, // test_mode
            res,
        )
    }
}

/// Requests the remote attestation of the client VM for testing.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `challenge` must be [valid] for reads of `challenge_size` bytes.
///
/// [valid]: ptr#safety
#[no_mangle]
pub unsafe extern "C" fn AVmPayload_requestAttestationForTesting(
    challenge: *const u8,
    challenge_size: usize,
    res: &mut *mut AttestationResult,
) -> AVmAttestationStatus {
    // SAFETY: The caller guarantees that `challenge` is valid for reads and `res` is valid
    // for writes.
    unsafe {
        request_attestation(
            challenge,
            challenge_size,
            true, // test_mode
            res,
        )
    }
}

/// Requests the remote attestation of the client VM.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `challenge` must be [valid] for reads of `challenge_size` bytes.
///
/// [valid]: ptr#safety
unsafe fn request_attestation(
    challenge: *const u8,
    challenge_size: usize,
    test_mode: bool,
    res: &mut *mut AttestationResult,
) -> AVmAttestationStatus {
    initialize_logging();
    const MAX_CHALLENGE_SIZE: usize = 64;
    if challenge_size > MAX_CHALLENGE_SIZE {
        return AVmAttestationStatus::ATTESTATION_ERROR_INVALID_CHALLENGE;
    }
    let challenge = if challenge_size == 0 {
        &[]
    } else {
        // SAFETY: The caller guarantees that `challenge` is valid for reads of
        // `challenge_size` bytes and `challenge_size` is not zero.
        unsafe { std::slice::from_raw_parts(challenge, challenge_size) }
    };
    let service = unwrap_or_abort(get_vm_payload_service());
    match service.requestAttestation(challenge, test_mode) {
        Ok(attestation_res) => {
            *res = Box::into_raw(Box::new(attestation_res));
            AVmAttestationStatus::ATTESTATION_OK
        }
        Err(e) => {
            error!("Remote attestation failed: {e:?}");
            binder_status_to_attestation_status(e)
        }
    }
}

fn binder_status_to_attestation_status(status: binder::Status) -> AVmAttestationStatus {
    match status.exception_code() {
        ExceptionCode::UNSUPPORTED_OPERATION => AVmAttestationStatus::ATTESTATION_ERROR_UNSUPPORTED,
        _ => AVmAttestationStatus::ATTESTATION_ERROR_ATTESTATION_FAILED,
    }
}

/// Converts the return value from `AVmPayload_requestAttestation` to a text string
/// representing the error code.
#[no_mangle]
pub extern "C" fn AVmAttestationStatus_toString(status: AVmAttestationStatus) -> *const c_char {
    let message = match status {
        AVmAttestationStatus::ATTESTATION_OK => {
            CStr::from_bytes_with_nul(b"The remote attestation completes successfully.\0").unwrap()
        }
        AVmAttestationStatus::ATTESTATION_ERROR_INVALID_CHALLENGE => {
            CStr::from_bytes_with_nul(b"The challenge size is not between 0 and 64.\0").unwrap()
        }
        AVmAttestationStatus::ATTESTATION_ERROR_ATTESTATION_FAILED => {
            CStr::from_bytes_with_nul(b"Failed to attest the VM. Please retry at a later time.\0")
                .unwrap()
        }
        AVmAttestationStatus::ATTESTATION_ERROR_UNSUPPORTED => CStr::from_bytes_with_nul(
            b"Remote attestation is not supported in the current environment.\0",
        )
        .unwrap(),
    };
    message.as_ptr()
}

/// Reads the DER-encoded ECPrivateKey structure specified in [RFC 5915 s3] for the
/// EC P-256 private key from the provided attestation result.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `data` must be [valid] for writes of `size` bytes, if size > 0.
/// * The region of memory beginning at `data` with `size` bytes must not overlap with the region of
///   memory `res` points to.
///
/// [valid]: ptr#safety
/// [RFC 5915 s3]: https://datatracker.ietf.org/doc/html/rfc5915#section-3
#[no_mangle]
pub unsafe extern "C" fn AVmAttestationResult_getPrivateKey(
    res: &AttestationResult,
    data: *mut u8,
    size: usize,
) -> usize {
    let private_key = &res.privateKey;
    if size != 0 {
        let data = NonNull::new(data).expect("data must not be null when size > 0");
        // SAFETY: See the requirements on `data` above. The number of bytes copied doesn't exceed
        // the length of either buffer, and the caller ensures that `private_key` cannot overlap
        // `data`. We allow data to be null, which is never valid, but only if size == 0
        // which is checked above.
        unsafe {
            ptr::copy_nonoverlapping(
                private_key.as_ptr(),
                data.as_ptr(),
                std::cmp::min(private_key.len(), size),
            )
        };
    }
    private_key.len()
}

/// Signs the given message using ECDSA P-256, the message is first hashed with SHA-256 and
/// then it is signed with the attested EC P-256 private key in the attestation result.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `message` must be [valid] for reads of `message_size` bytes.
/// * `data` must be [valid] for writes of `size` bytes, if size > 0.
/// * The region of memory beginning at `data` with `size` bytes must not overlap with the region of
///   memory `res` or `message` point to.
///
///
/// [valid]: ptr#safety
#[no_mangle]
pub unsafe extern "C" fn AVmAttestationResult_sign(
    res: &AttestationResult,
    message: *const u8,
    message_size: usize,
    data: *mut u8,
    size: usize,
) -> usize {
    // A DER-encoded ECDSA signature can have varying sizes even with the same EC Key and message,
    // due to the encoding of the random values r and s that are part of the signature.
    if size == 0 {
        return MAX_ECDSA_P256_SIGNATURE_SIZE;
    }
    if message_size == 0 {
        panic!("Message to be signed must not be empty.")
    }
    // SAFETY: See the requirements on `message` above.
    let message = unsafe { std::slice::from_raw_parts(message, message_size) };
    let signature = unwrap_or_abort(try_ecdsa_sign(message, &res.privateKey));
    let data = NonNull::new(data).expect("data must not be null when size > 0");
    // SAFETY: See the requirements on `data` above. The number of bytes copied doesn't exceed
    // the length of either buffer, and the caller ensures that `signature` cannot overlap
    // `data`. We allow data to be null, which is never valid, but only if size == 0
    // which is checked above.
    unsafe {
        ptr::copy_nonoverlapping(
            signature.as_ptr(),
            data.as_ptr(),
            usize::min(signature.len(), size),
        )
    };
    if size < signature.len() {
        // If the buffer is too small, return the maximum size of the signature to allow the caller
        // to allocate a buffer large enough to call this function again.
        MAX_ECDSA_P256_SIGNATURE_SIZE
    } else {
        signature.len()
    }
}

fn try_ecdsa_sign(message: &[u8], der_encoded_ec_private_key: &[u8]) -> Result<Vec<u8>> {
    let private_key = EcKey::private_key_from_der(der_encoded_ec_private_key)?;
    let digest = sha256(message);
    let sig = EcdsaSig::sign(&digest, &private_key)?;
    Ok(sig.to_der()?)
}

/// Gets the number of certificates in the certificate chain.
#[no_mangle]
pub extern "C" fn AVmAttestationResult_getCertificateCount(res: &AttestationResult) -> usize {
    res.certificateChain.len()
}

/// Retrieves the certificate at the given `index` from the certificate chain in the provided
/// attestation result.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `data` must be [valid] for writes of `size` bytes, if size > 0.
/// * `index` must be within the range of [0, number of certificates). The number of certificates
///   can be obtained with `AVmAttestationResult_getCertificateCount`.
/// * The region of memory beginning at `data` with `size` bytes must not overlap with the region of
///   memory `res` points to.
///
/// [valid]: ptr#safety
#[no_mangle]
pub unsafe extern "C" fn AVmAttestationResult_getCertificateAt(
    res: &AttestationResult,
    index: usize,
    data: *mut u8,
    size: usize,
) -> usize {
    let certificate =
        &res.certificateChain.get(index).expect("The index is out of bounds.").encodedCertificate;
    if size != 0 {
        let data = NonNull::new(data).expect("data must not be null when size > 0");
        // SAFETY: See the requirements on `data` above. The number of bytes copied doesn't exceed
        // the length of either buffer, and the caller ensures that `certificate` cannot overlap
        // `data`. We allow data to be null, which is never valid, but only if size == 0
        // which is checked above.
        unsafe {
            ptr::copy_nonoverlapping(
                certificate.as_ptr(),
                data.as_ptr(),
                std::cmp::min(certificate.len(), size),
            )
        };
    }
    certificate.len()
}

/// Frees all the data owned by given attestation result and result itself.
///
/// # Safety
///
/// Behavior is undefined if any of the following conditions are violated:
///
/// * `res` must point to a valid `AttestationResult` and has not been freed before.
#[no_mangle]
pub unsafe extern "C" fn AVmAttestationResult_free(res: *mut AttestationResult) {
    if !res.is_null() {
        // SAFETY: The result is only freed once is ensured by the caller.
        let res = unsafe { Box::from_raw(res) };
        drop(res)
    }
}

/// Gets the path to the APK contents.
#[no_mangle]
pub extern "C" fn AVmPayload_getApkContentsPath() -> *const c_char {
    VM_APK_CONTENTS_PATH_C.as_ptr()
}

/// Gets the path to the VM's encrypted storage.
#[no_mangle]
pub extern "C" fn AVmPayload_getEncryptedStoragePath() -> *const c_char {
    if Path::new(ENCRYPTEDSTORE_MOUNTPOINT).exists() {
        VM_ENCRYPTED_STORAGE_PATH_C.as_ptr()
    } else {
        ptr::null()
    }
}
