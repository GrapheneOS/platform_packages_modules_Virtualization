// Copyright (C) 2025 The Android Open Source Project
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! This crate provides a C-compatible Foreign Function Interface (FFI) for performing
//! match rule verification on CompOS generated manifests.
//! It allows C/C++ clients (artd) to create an expectation context and specify what compiler
//! arguments to compare against.

use crate::wrappers;
use anyhow::{Context, Result};
use bssl_crypto::digest;
use bssl_crypto::ed25519;
use compos_bindgen::{
    AVerifiedDex2Oat_Verifier_ExpectationContext as FFIExpectationContext,
    AVerifiedDex2Oat_Verifier_Status as FFIVerifierStatus,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_BAD_ARGS as FFIVerifierStatus_BAD_ARGS,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_EXACT_MATCH_FAILED as FFIVerifierStatus_EXACT_MATCH_FAILED,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_FAILURE as FFIVerifierStatus_FAILURE,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_FILE_MISMATCHED_FS_VERITY_DIGEST as FFIVerifierStatus_FILE_MISMATCHED_FS_VERITY_DIGEST,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_FILE_MISSING_FS_VERITY_DIGEST as FFIVerifierStatus_FILE_MISSING_FS_VERITY_DIGEST,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_INVALID_MANIFEST_SIGNATURE as FFIVerifierStatus_INVALID_MANIFEST_SIGNATURE,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_INVALID_STATE as FFIVerifierStatus_INVALID_STATE,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_PROHIBITED_ARGUMENT as FFIVerifierStatus_PROHIBITED_ARGUMENT,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_SUCCESS as FFIVerifierStatus_SUCCESS,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_UNABLE_TO_OPEN_MANIFEST as FFIVerifierStatus_UNABLE_TO_OPEN_MANIFEST,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_UNABLE_TO_RETRIEVE_PUBLIC_KEY as FFIVerifierStatus_UNABLE_TO_RETRIEVE_PUBLIC_KEY,
    AVerifiedDex2Oat_Verifier_Status_AVERIFIED_DEX2OAT_VERIFIER_UNEXPECTED_ARGUMENT as FFIVerifierStatus_UNEXPECTED_ARGUMENT,
};
use compos_common::COMPOS_MANIFEST_MAGIC_PREFIX;
use compos_manifest_proto::manifest::signature::signed_manifest::secure_compile_manifest::CompilerArgument;
use compos_manifest_proto::manifest::signature::Signature as SignatureOneof;
use compos_manifest_proto::manifest::Signature;
use compos_manifest_proto::manifest::SignatureAlgorithm;
#[cfg(not(test))]
use compos_wrappers::fsverity;
#[cfg(test)]
use compos_wrappers_with_mocks::mock_fsverity as fsverity;
use hex;
use libc;
use log::error;
use protobuf::Message;
use std::ffi::{c_char, CStr, CString};
use std::os::fd::BorrowedFd;
use std::ptr;
use std::slice;

const DEX2OAT_PUBLIC_KEY_PATH: &str = "/data/misc/apexdata/com.android.compos/compos_dex2oat_key";

// Macro to assert that a pointer is not null.
macro_rules! assert_not_null {
    ($($arg:expr),+ $(,)?) => {
        $(
            assert!(!$arg.is_null(), concat!(stringify!($arg), " must not be null"));
        )+
    };
}

// Macro to set the error message and return the status.
macro_rules! set_error_and_return {
    ($context:expr, $status:expr, $msg:expr) => {
        $context.error_message = Some(CString::new($msg.to_string()).unwrap());
        return $status;
    };
}

const ED25519_PUBLIC_KEY_LEN: usize = 32;

#[derive(PartialEq, Copy, Clone, Debug)]
enum ArgumentType {
    Ignored,
    Disallowed,
    Exact,
}

struct Argument {
    // The compiler argument.
    flag: String,
    // Should the compiler argument match if it starts with the flag.
    is_prefix: bool,
    // fs-verity hashes of associated files.
    fs_verity_hashes: Vec<Vec<u8>>,
}

struct Rule {
    // The list of arguments to match.
    args: Vec<Argument>,
    // See ArgumentType enum.
    arg_type: ArgumentType,
}

// Holds the state and resources for verifying a CompOS manifest.
#[repr(C)]
struct ExpectationContext {
    // Argument rules to be verified against manifest.
    rules: Vec<Rule>,
    // The path to the dex2oat public key.
    dex2oat_public_key_path: String,
    // The C-style error message from the last verification attempt.
    // This is stored as a CString to ensure it is null-terminated and its lifetime is tied to
    // the context.
    error_message: Option<CString>,
}

// Try to read the fs-verity digest from fs-verity and if that fails fall back
// to reading it from the appropriate fsv_meta file.
fn read_digest_from_verity_or_fsv_meta(fd: BorrowedFd) -> Result<Vec<u8>> {
    match fsverity::read_digest(fd) {
        Ok(result) => Ok(result.to_vec()),
        Err(e) => {
            error!("fsverity::read_digest failed: {e:?}, falling back to fsv_meta");
            let digest = fsverity::read_digest_from_fsv_meta(fd)
                .context("fall back to read from fsv_meta failed.")?;
            Ok(digest.to_vec())
        }
    }
}

// Retrieves the public key that will be used to verify manifest signature.
fn get_composd_public_key(
    dex2oat_public_key_path: &str,
) -> Result<[u8; ED25519_PUBLIC_KEY_LEN], FFIVerifierStatus> {
    let path = std::path::Path::new(dex2oat_public_key_path);
    if !path.exists() {
        error!("dex2oat_public_key_path does not exist: {:?}", path);
        return Err(FFIVerifierStatus_UNABLE_TO_RETRIEVE_PUBLIC_KEY);
    }

    let key_vec = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            error!("Failed to read dex2oat_public_key_path: {:?}", path);
            return Err(FFIVerifierStatus_UNABLE_TO_RETRIEVE_PUBLIC_KEY);
        }
    };

    if key_vec.len() != ED25519_PUBLIC_KEY_LEN {
        error!("Public key has unexpected length: {}", key_vec.len());
        return Err(FFIVerifierStatus_UNABLE_TO_RETRIEVE_PUBLIC_KEY);
    }
    let mut key_array = [0u8; ED25519_PUBLIC_KEY_LEN];
    key_array.copy_from_slice(&key_vec);
    Ok(key_array)
}

/// Creates and initializes a expectation context for checking against a CompOS manifest.
///
/// Refer to the public C API header for the full documentation.
///
/// # Safety
/// The caller is responsible for managing the lifetime of the returned context.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_Verifier_Expectation_create() -> *mut FFIExpectationContext
{
    let context = Box::new(ExpectationContext {
        rules: Vec::new(),
        dex2oat_public_key_path: DEX2OAT_PUBLIC_KEY_PATH.to_string(),
        error_message: None,
    });
    Box::into_raw(context) as *mut FFIExpectationContext
}

/// Destroys the expectation context.
///
/// Refer to the public C API header for the full documentation.
///
/// # Safety
/// The caller must ensure that the context is valid and was created by
/// AVerifiedDex2Oat_Verifier_Expectation_create, and that it is not used after this call.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_Verifier_Expectation_destroy(
    ctx: *mut FFIExpectationContext,
) {
    assert_not_null!(ctx);
    // SAFETY: The caller ensures ctx is a valid pointer to an ExpectationContext that was leaked
    // via Box::into_raw.
    unsafe {
        let _ = Box::from_raw(ctx as *mut ExpectationContext);
    }
}

/// Returns the error message (or null) from the last verification attempt.
///
/// Refer to the public C API header for the full documentation.
///
/// # Safety
/// The caller must ensure that ctx is valid and was created by
/// AVerifiedDex2Oat_Verifier_Expectation_create.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(
    ctx: *mut FFIExpectationContext,
) -> *const c_char {
    assert_not_null!(ctx);
    // SAFETY: Caller ensures ctx is a valid pointer to a boxed ExpectationContext by
    // AVerifiedDex2Oat_Verifier_Expectation_create.
    let context = unsafe { &*(ctx as *mut ExpectationContext) };

    context.error_message.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null())
}

/// Adds an exact match rule for a compiler argument.
///
/// Refer to the public C API header for the full documentation.
///
/// # Safety
/// The caller must ensure that format_string is a valid null-terminated C string and fds is valid
/// for fd_count elements. ctx must be valid and created by
/// AVerifiedDex2Oat_Verifier_Expectation_create.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(
    ctx: *mut FFIExpectationContext,
    format_string: *const c_char,
    fds: *const i32,
    fd_count: usize,
) -> FFIVerifierStatus {
    assert_not_null!(ctx);
    assert_not_null!(format_string);
    if fd_count > 0 {
        assert!(
            !fds.is_null(),
            "AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule: fds is null"
        );
    }

    // SAFETY: Caller ensures ctx is a valid pointer to a boxed ExpectationContext by
    // AVerifiedDex2Oat_Verifier_Expectation_create.
    let context = unsafe { &mut *(ctx as *mut ExpectationContext) };

    // SAFETY: Caller ensures format_string is a valid null-terminated C string.
    let format_str = unsafe {
        match CStr::from_ptr(format_string).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return FFIVerifierStatus_BAD_ARGS,
        }
    };

    if wrappers::count_placeholders(&format_str) != fd_count as u32 {
        return FFIVerifierStatus_BAD_ARGS;
    }

    // SAFETY: Caller ensures fds is valid for fd_count elements.
    let fds_slice = unsafe { slice::from_raw_parts(fds, fd_count) };
    let mut hashes = Vec::new();
    for &fd in fds_slice {
        // SAFETY: F_GETFD is a read-only operation that will not change the program state. Any
        // value of fd should be safe since an invalid file descriptor will result in a `-1`
        // return value.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 {
            return FFIVerifierStatus_BAD_ARGS;
        }
        // SAFETY: fd is checked for validity above.
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
        match read_digest_from_verity_or_fsv_meta(borrowed_fd) {
            Ok(hash) => hashes.push(hash),
            Err(e) => {
                error!("Failed to retrieve fs-verity digest: {:?}", e);
                return FFIVerifierStatus_FILE_MISSING_FS_VERITY_DIGEST;
            }
        }
    }
    let argument = Argument { flag: format_str, is_prefix: false, fs_verity_hashes: hashes };
    context.rules.push(Rule { args: vec![argument], arg_type: ArgumentType::Exact });
    FFIVerifierStatus_SUCCESS
}

/// Adds a disallowed argument rule.
///
/// Refer to the public C API header for the full documentation.
///
/// # Safety
/// The caller must ensure that compiler_arg is a valid null-terminated C string and ctx is valid
/// and was created by AVerifiedDex2Oat_Verifier_Expectation_create.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_Verifier_Expectation_addDisallowedArgumentRule(
    ctx: *mut FFIExpectationContext,
    compiler_arg: *const c_char,
    is_prefix: bool,
) -> FFIVerifierStatus {
    assert_not_null!(ctx);
    assert_not_null!(compiler_arg);

    // SAFETY: Caller ensures ctx is a valid pointer to a boxed ExpectationContext by
    // AVerifiedDex2Oat_Verifier_Expectation_create.
    let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
    // SAFETY: Caller ensures compiler_arg is a valid null-terminated C string.
    let arg_str = unsafe {
        match CStr::from_ptr(compiler_arg).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return FFIVerifierStatus_BAD_ARGS,
        }
    };
    let argument = Argument { flag: arg_str, is_prefix, fs_verity_hashes: Vec::new() };
    context.rules.push(Rule { args: vec![argument], arg_type: ArgumentType::Disallowed });
    FFIVerifierStatus_SUCCESS
}

/// Combines the current argument with the previous match rule.
///
/// Refer to the public C API header for the full documentation.
///
/// # Safety
/// The caller must ensure that compiler_arg is a valid null-terminated C string and fds is valid
/// for fd_count elements. ctx is valid and was created by
/// AVerifiedDex2Oat_Verifier_Expectation_create.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(
    ctx: *mut FFIExpectationContext,
    compiler_arg: *const c_char,
    is_prefix: bool,
) -> FFIVerifierStatus {
    assert_not_null!(ctx);
    assert_not_null!(compiler_arg);

    // SAFETY: Caller ensures ctx is a valid pointer to a boxed ExpectationContext by
    // AVerifiedDex2Oat_Verifier_Expectation_create.
    let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
    if context.rules.is_empty() {
        return FFIVerifierStatus_INVALID_STATE;
    }
    if let Some(last_rule) = context.rules.last_mut() {
        if last_rule.arg_type != ArgumentType::Exact {
            // SAFETY: Caller ensures compiler_arg is a valid null-terminated C string.
            let compiler_arg_str = unsafe {
                match CStr::from_ptr(compiler_arg).to_str() {
                    Ok(s) => s.to_string(),
                    Err(_) => return FFIVerifierStatus_BAD_ARGS,
                }
            };
            let argument =
                Argument { flag: compiler_arg_str, is_prefix, fs_verity_hashes: Vec::new() };
            last_rule.args.push(argument);
        } else {
            // The previous argument needs to be ignored or disallowed.
            return FFIVerifierStatus_INVALID_STATE;
        }
    }
    FFIVerifierStatus_SUCCESS
}

/// Adds an ignored argument rule.
///
/// Refer to the public C API header for the full documentation.
///
/// # Safety
/// The caller must ensure that compiler_arg is a valid null-terminated C string and ctx is valid
/// and was created by AVerifiedDex2Oat_Verifier_Expectation_create.
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(
    ctx: *mut FFIExpectationContext,
    compiler_arg: *const c_char,
    is_prefix: bool,
) -> FFIVerifierStatus {
    assert_not_null!(ctx);
    assert_not_null!(compiler_arg);

    // SAFETY: Caller ensures ctx is a valid pointer to a boxed ExpectationContext by
    // AVerifiedDex2Oat_Verifier_Expectation_create.
    let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
    // SAFETY: Caller ensures compiler_arg is a valid null-terminated C string.
    let arg_str = unsafe {
        match CStr::from_ptr(compiler_arg).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return FFIVerifierStatus_BAD_ARGS,
        }
    };
    let argument = Argument { flag: arg_str, is_prefix, fs_verity_hashes: Vec::new() };
    context.rules.push(Rule { args: vec![argument], arg_type: ArgumentType::Ignored });
    FFIVerifierStatus_SUCCESS
}

fn check_args_match(
    rule_args: &[Argument],
    manifest_args: &[CompilerArgument],
) -> Result<Option<String>, String> {
    if rule_args.len() > manifest_args.len() {
        return Ok(Some(format!(
            "Rule expects {} arguments, but only {} remaining in manifest",
            rule_args.len(),
            manifest_args.len()
        )));
    }
    for i in 0..rule_args.len() {
        let rule_arg = &rule_args[i];
        let manifest_arg = &manifest_args[i];

        let Some(ref current_flag) = manifest_arg.compiler_flag else {
            return Err("Manifest arg malformed. No compiler_flag present".to_string());
        };

        let arg_matches = if rule_arg.is_prefix {
            current_flag.starts_with(&rule_arg.flag)
        } else {
            current_flag == &rule_arg.flag
        };

        if !arg_matches {
            return Ok(Some(format!(
                "Rule not matched: expected {}, got {}",
                rule_arg.flag, current_flag
            )));
        }
    }
    Ok(None)
}

fn check_fsverity_digests(
    rule_args: &[Argument],
    manifest_args: &[CompilerArgument],
) -> Result<(), String> {
    for (rule_arg, manifest_arg) in rule_args.iter().zip(manifest_args.iter()) {
        if rule_arg.fs_verity_hashes.len() != manifest_arg.file_info.len() {
            return Err(format!(
                "fs-verity hash count mismatch for {}: expected {}, got {}",
                rule_arg.flag,
                rule_arg.fs_verity_hashes.len(),
                manifest_arg.file_info.len()
            ));
        }

        for (expected_hash, file_details) in
            rule_arg.fs_verity_hashes.iter().zip(manifest_arg.file_info.iter())
        {
            let Some(ref expected_digest_hex) = file_details.verity_digest else {
                return Err(format!("Missing fs-verity digest in manifest for {}", rule_arg.flag));
            };
            // Digest hash is prepended with "sha256-".
            let actual_digest_hex = format!("sha256-{}", hex::encode(expected_hash));
            if expected_digest_hex != &actual_digest_hex {
                return Err(format!(
                    "fs-verity digest mismatch for {}: expected {}, got {}",
                    rule_arg.flag, actual_digest_hex, expected_digest_hex
                ));
            }
        }
    }
    Ok(())
}

/// Verifies the given expectation rules against the manifest file.
///
/// Refer to the public C API header for the full documentation.
///
/// # Safety
/// The caller must ensure that manifest_path is a valid null-terminated UTF-8 string and ctx is
/// valid and was created by AVerifiedDex2Oat_Verifier_Expectation_create..
#[no_mangle]
pub unsafe extern "C" fn AVerifiedDex2Oat_Verifier_Expectation_verify(
    ctx: *mut FFIExpectationContext,
    manifest_path: *const c_char,
) -> FFIVerifierStatus {
    assert_not_null!(ctx);
    assert_not_null!(manifest_path);

    // SAFETY: Caller ensures ctx is a valid pointer to a boxed ExpectationContext by
    // AVerifiedDex2Oat_Verifier_Expectation_create.
    let context = unsafe { &mut *(ctx as *mut ExpectationContext) };

    // Clear previous error.
    context.error_message = None;

    let public_key = match get_composd_public_key(&context.dex2oat_public_key_path) {
        Ok(key) => key,
        Err(status) => {
            set_error_and_return!(context, status, "Unable to retrieve CompOS app VM public key");
        }
    };

    // SAFETY: The caller ensures manifest_path is a valid null-terminated UTF-8 string.
    let path_str = unsafe { CStr::from_ptr(manifest_path) };
    let path = match path_str.to_str() {
        Ok(s) => std::path::Path::new(s),
        Err(_) => {
            set_error_and_return!(
                context,
                FFIVerifierStatus_BAD_ARGS,
                "Manifest path is not valid UTF-8"
            );
        }
    };
    if !path.exists() {
        set_error_and_return!(
            context,
            FFIVerifierStatus_UNABLE_TO_OPEN_MANIFEST,
            "manifest could not be opened"
        );
    }

    // Read manifest.
    let manifest_bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            set_error_and_return!(
                context,
                FFIVerifierStatus_UNABLE_TO_OPEN_MANIFEST,
                "manifest could not be opened"
            );
        }
    };
    let compos_manifest_proto = match Signature::parse_from_bytes(&manifest_bytes) {
        Ok(proto) => proto,
        Err(_) => {
            set_error_and_return!(
                context,
                FFIVerifierStatus_UNABLE_TO_OPEN_MANIFEST,
                "manifest could not be opened"
            );
        }
    };

    // Verify signature.
    let signature_oneof = if let Some(ref signature) = compos_manifest_proto.signature {
        signature
    } else {
        set_error_and_return!(
            context,
            FFIVerifierStatus_FAILURE,
            "could not find signature in proto"
        );
    };

    let signed_manifest =
        if let SignatureOneof::ComposSignedManifest(ref manifest) = signature_oneof {
            manifest
        } else {
            set_error_and_return!(
                context,
                FFIVerifierStatus_FAILURE,
                "could not find signed manifest in signature"
            );
        };

    let manifest_field = signed_manifest.manifest.as_ref();
    let secure_manifest = if let Some(manifest_payload) = manifest_field {
        manifest_payload
    } else {
        set_error_and_return!(
            context,
            FFIVerifierStatus_FAILURE,
            "signed manifest missing payload"
        );
    };

    if signed_manifest.algorithm.unwrap() != SignatureAlgorithm::ED25519.into() {
        set_error_and_return!(
            context,
            FFIVerifierStatus_INVALID_MANIFEST_SIGNATURE,
            "manifest signature invalid"
        );
    }

    let secure_manifest_bytes = secure_manifest.write_to_bytes().unwrap();
    let digest = digest::Sha256::hash(&secure_manifest_bytes);
    // Use the "magic word" to prepend the sha256 hash of the manifest bytes. Then verify the
    // signature.
    let msg_to_verify = [COMPOS_MANIFEST_MAGIC_PREFIX.as_bytes(), &digest].concat();

    let public_key = ed25519::PublicKey::from_bytes(&public_key);
    let signature_bytes = signed_manifest.signature.as_ref().unwrap();
    let Ok(signature) = signature_bytes.as_slice().try_into() else {
        set_error_and_return!(
            context,
            FFIVerifierStatus_INVALID_MANIFEST_SIGNATURE,
            "manifest signature could not be converted to byte slice"
        );
    };

    if let Err(e) = public_key.verify(&msg_to_verify, &signature) {
        set_error_and_return!(
            context,
            FFIVerifierStatus_INVALID_MANIFEST_SIGNATURE,
            format!("manifest signature invalid: {e:?}")
        );
    }

    // Verify compiler args.
    let mut manifest_iter = secure_manifest.compiler_arguments.iter();

    for rule in context.rules.iter() {
        // remaining_slice represents the manifest arguments that have not been checked yet.
        let remaining_slice = manifest_iter.as_slice();

        match rule.arg_type {
            // Disallowed rules check if the UPCOMING manifest argument(s) matches
            // the rule's argument(s). If all of the argument(s) match, it is an
            // PROHIBITED_ARGUMENT error.
            // If the argument(s) do not match it will go to the next rule and check the same
            // argument(s) for verification again.
            ArgumentType::Disallowed => match check_args_match(&rule.args, remaining_slice) {
                Ok(None) => {
                    set_error_and_return!(
                        context,
                        FFIVerifierStatus_PROHIBITED_ARGUMENT,
                        format!("Disallowed argument rule {{ {} }} failed", rule.args[0].flag)
                    );
                }
                Ok(Some(_msg)) => {
                    continue;
                }
                Err(msg) => {
                    set_error_and_return!(context, FFIVerifierStatus_FAILURE, msg);
                }
            },
            // Ignored mostly follows same logic as Disallowed but will not fail on a match.
            ArgumentType::Ignored => match check_args_match(&rule.args, remaining_slice) {
                Ok(None) => {
                    // Advance the iterator to consume the matched arguments.
                    for _ in 0..rule.args.len() {
                        manifest_iter.next();
                    }
                }
                Ok(Some(_msg)) => {
                    continue;
                }
                Err(msg) => {
                    set_error_and_return!(context, FFIVerifierStatus_FAILURE, msg);
                }
            },
            ArgumentType::Exact => match check_args_match(&rule.args, remaining_slice) {
                Ok(None) => {
                    // Advance the iterator to consume the matched arguments.
                    for _ in 0..rule.args.len() {
                        manifest_iter.next();
                    }

                    // Verify fs-verity digests.
                    if let Err(msg) = check_fsverity_digests(&rule.args, remaining_slice) {
                        set_error_and_return!(
                            context,
                            FFIVerifierStatus_FILE_MISMATCHED_FS_VERITY_DIGEST,
                            msg
                        );
                    }
                }
                // The args did not match but no error occurred.
                Ok(Some(msg)) => {
                    set_error_and_return!(context, FFIVerifierStatus_EXACT_MATCH_FAILED, msg);
                }
                Err(msg) => {
                    set_error_and_return!(context, FFIVerifierStatus_FAILURE, msg);
                }
            },
        }
    }

    if let Some(arg) = manifest_iter.next() {
        set_error_and_return!(
            context,
            FFIVerifierStatus_UNEXPECTED_ARGUMENT,
            format!("Unmatched argument in manifest: {}", arg.compiler_flag.as_ref().unwrap())
        );
    }

    FFIVerifierStatus_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use compos_manifest_proto::manifest::signature::signed_manifest::secure_compile_manifest::CompilerArgument;
    use compos_manifest_proto::manifest::signature::signed_manifest::SecureCompileManifest as Manifest;
    use compos_manifest_proto::manifest::signature::SignedManifest;
    use std::os::fd::AsRawFd;
    use std::ptr;
    use tempfile;

    use bssl_crypto::ed25519;
    use protobuf::Message;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn generate_keypair_and_write_public_key() -> (ed25519::PrivateKey, NamedTempFile) {
        let key_pair = ed25519::PrivateKey::generate();
        let public_key_bytes = key_pair.to_public();

        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile.write_all(public_key_bytes.as_bytes()).unwrap();
        (key_pair, tmpfile)
    }

    fn setup_test_context(
    ) -> (*mut FFIExpectationContext, NamedTempFile, NamedTempFile, CString, ed25519::PrivateKey)
    {
        // SAFETY: The function AVerifiedDex2Oat_Verifier_Expectation_create is a C function
        // that returns a pointer to a new ExpectationContext.
        let ctx = unsafe { AVerifiedDex2Oat_Verifier_Expectation_create() };
        assert_not_null!(ctx);
        let (key_pair, tmp_pubkey_file) = generate_keypair_and_write_public_key();
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
        context.dex2oat_public_key_path = tmp_pubkey_file.path().to_str().unwrap().to_string();

        let tmp_manifest = NamedTempFile::new().unwrap();
        let manifest_path_cstr = CString::new(tmp_manifest.path().to_str().unwrap()).unwrap();

        (ctx, tmp_pubkey_file, tmp_manifest, manifest_path_cstr, key_pair)
    }

    fn create_dummy_manifest(
        path: &str,
        compiler_args: &[CompilerArgument],
        key_pair: &ed25519::PrivateKey,
    ) {
        let mut manifest = Manifest::new();
        manifest.compiler_arguments = compiler_args.to_vec();

        let mut signed_manifest = SignedManifest::new();
        signed_manifest.algorithm = Some(SignatureAlgorithm::ED25519.into());
        signed_manifest.manifest = protobuf::MessageField::some(manifest);

        // Sign the manifest with the provided key.
        let manifest_bytes = signed_manifest.manifest.as_ref().unwrap().write_to_bytes().unwrap();
        let digest = bssl_crypto::digest::Sha256::hash(&manifest_bytes);
        let msg_to_sign = [COMPOS_MANIFEST_MAGIC_PREFIX.as_bytes(), &digest].concat();
        let signature = key_pair.sign(&msg_to_sign);
        signed_manifest.signature = Some(signature.to_vec());

        let mut signature_proto = Signature::new();
        signature_proto.signature = Some(SignatureOneof::ComposSignedManifest(signed_manifest));

        let manifest_bytes = signature_proto.write_to_bytes().unwrap();
        std::fs::write(path, manifest_bytes).unwrap();
    }

    #[test]
    fn test_create_destroy_expectation() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, _manifest_path, _key_pair) =
            setup_test_context();

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_add_exact_match_rule() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, _manifest_path, _key_pair) =
            setup_test_context();

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
        assert!(context.rules.is_empty());

        let arg = CString::new("-Xdexopt:verified").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg.as_ptr() is a valid pointer
        // to a null-terminated C string.
        let status = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(
                ctx,
                arg.as_ptr(),
                ptr::null_mut(),
                0,
            )
        };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
        assert_eq!(context.rules.len(), 1);
        assert_eq!(context.rules[0].args.len(), 1);
        assert_eq!(context.rules[0].arg_type, ArgumentType::Exact);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_add_exact_match_rule_with_fds() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, _manifest_path, _key_pair) =
            setup_test_context();

        let dummy_digest = [0u8; 32];
        let _read_digest_ctx = {
            let ctx = fsverity::read_digest_context();
            // First call fails, triggers fallback.
            ctx.expect().times(1).returning(|_| Err(std::io::Error::other("Not in fs-verity")));
            ctx
        };

        let _read_fsv_meta_ctx = {
            let ctx = fsverity::read_digest_from_fsv_meta_context();
            ctx.expect().times(1).returning(move |_| Ok(dummy_digest));
            ctx
        };

        let format_string = CString::new("FormatString!").unwrap();
        let file = tempfile::tempfile().unwrap();
        let fds = [file.as_raw_fd()];

        // SAFETY: ctx is a valid pointer to an ExpectationContext, format_string.as_ptr() is a
        // valid pointer to a null-terminated C string, fds.as_ptr() is a valid pointer to an
        // array of i32s.
        let status = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(
                ctx,
                format_string.as_ptr(),
                fds.as_ptr(),
                fds.len(),
            )
        };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
        assert_eq!(context.rules.len(), 1);
        assert_eq!(context.rules[0].args[0].fs_verity_hashes[0], dummy_digest.to_vec());

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_add_disallowed_argument_rule() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, _manifest_path, _key_pair) =
            setup_test_context();

        let arg = CString::new("--input-vdex-fd=-1").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg.as_ptr() is a valid pointer
        // to a null-terminated C string.
        let status = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addDisallowedArgumentRule(ctx, arg.as_ptr(), true)
        };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
        assert_eq!(context.rules.len(), 1);
        assert_eq!(context.rules[0].args.len(), 1);
        assert_eq!(context.rules[0].arg_type, ArgumentType::Disallowed);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_add_ignored_argument_rule() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, _manifest_path, _key_pair) =
            setup_test_context();
        let arg = CString::new("--ignore-me").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg.as_ptr() is a valid pointer
        // to a null-terminated C string.
        let status = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(ctx, arg.as_ptr(), false)
        };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
        assert_eq!(context.rules.len(), 1);
        assert_eq!(context.rules[0].args.len(), 1);
        assert_eq!(context.rules[0].arg_type, ArgumentType::Ignored);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_combine_with_previous_match_rule() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, _manifest_path, _key_pair) =
            setup_test_context();

        let arg1 = CString::new("--no_blah").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg1.as_ptr() is a valid pointer
        // to a null-terminated C string.
        let status = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(ctx, arg1.as_ptr(), false)
        };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        let arg2 = CString::new("--blah").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg2.as_ptr() is a valid pointer
        // to a null-terminated C string.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(
                ctx,
                arg2.as_ptr(),
                false,
            )
        };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let context = unsafe { &mut *(ctx as *mut ExpectationContext) };
        assert_eq!(context.rules.len(), 1);
        assert_eq!(context.rules[0].args.len(), 2);
        assert_eq!(context.rules[0].arg_type, ArgumentType::Ignored);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_verify_expectation_no_manifest() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, _manifest_path, _key_pair) =
            setup_test_context();

        let manifest_path = CString::new("non_existent_manifest.bin").unwrap();

        // SAFETY: ctx is a valid pointer to an ExpectationContext, manifest_path.as_ptr() is a
        // valid pointer to a null-terminated C string.
        let status =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_verify(ctx, manifest_path.as_ptr()) };
        assert_eq!(status, FFIVerifierStatus_UNABLE_TO_OPEN_MANIFEST);
        // SAFETY: ctx is a valid pointer.
        let error_message = unsafe { AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx) };
        assert_eq!(
            // SAFETY: error_message is either null or a valid pointer from the C API.
            unsafe { CStr::from_ptr(error_message) }.to_str().unwrap(),
            "manifest could not be opened"
        );
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_verify_expectation_unmatched_argument() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, manifest_path, key_pair) = setup_test_context();

        let arg = CString::new("-Xexact").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg.as_ptr() is a valid pointer
        // to a null-terminated C string.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(
                ctx,
                arg.as_ptr(),
                ptr::null_mut(),
                0,
            )
        };

        // Manifest has a different argument.
        let mut compiler_arg = CompilerArgument::new();
        compiler_arg.compiler_flag = Some("-Xunmatched".to_string());
        let compiler_args = vec![compiler_arg];

        create_dummy_manifest(manifest_path.to_str().unwrap(), &compiler_args, &key_pair);

        // SAFETY: ctx is a valid pointer to an ExpectationContext, manifest_path.as_ptr() is a
        // valid pointer to a null-terminated C string.
        let status =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_verify(ctx, manifest_path.as_ptr()) };
        assert_eq!(status, FFIVerifierStatus_EXACT_MATCH_FAILED);
        // SAFETY: ctx is a valid pointer.
        let error_message = unsafe { AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx) };
        assert_eq!(
            // SAFETY: error_message is either null or a valid pointer from the C API.
            unsafe { CStr::from_ptr(error_message) }.to_str().unwrap(),
            "Rule not matched: expected -Xexact, got -Xunmatched"
        );
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_verify_expectation_exact_match_not_found() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, manifest_path, key_pair) = setup_test_context();

        // Rule: Exact match for "-Xexact"
        let arg = CString::new("-Xexact").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg.as_ptr() is a valid pointer
        // to a null-terminated C string.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(
                ctx,
                arg.as_ptr(),
                ptr::null_mut(),
                0,
            )
        };

        create_dummy_manifest(manifest_path.to_str().unwrap(), &[], &key_pair);

        // SAFETY: ctx is a valid pointer to an ExpectationContext, manifest_path.as_ptr() is a
        // valid pointer to a null-terminated C string.
        let status =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_verify(ctx, manifest_path.as_ptr()) };
        assert_eq!(status, FFIVerifierStatus_EXACT_MATCH_FAILED);
        // SAFETY: ctx is a valid pointer.
        let error_message = unsafe { AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx) };
        assert_eq!(
            // SAFETY: error_message is either null or a valid pointer from the C API.
            unsafe { CStr::from_ptr(error_message) }.to_str().unwrap(),
            "Rule expects 1 arguments, but only 0 remaining in manifest"
        );
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_verify_expectation_success() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, manifest_path, key_pair) = setup_test_context();

        // Exact match without fds.
        let arg1 = CString::new("-Xexact1").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg1.as_ptr() is a valid pointer
        // to a null-terminated C string.
        let status = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(
                ctx,
                arg1.as_ptr(),
                ptr::null_mut(),
                0,
            )
        };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // Ignored argument rule
        let arg2 = CString::new("--ignored1").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg2.as_ptr() is a valid pointer
        // to a null-terminated C string.
        let status = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(ctx, arg2.as_ptr(), false)
        };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // Combine with previous ignored argument rule
        let arg3 = CString::new("--ignored2").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext, arg3.as_ptr() is a valid pointer
        // to a null-terminated C string.
        let status = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(
                ctx,
                arg3.as_ptr(),
                false,
            )
        };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // Create a manifest that should pass verification
        let mut compiler_args = Vec::new();
        compiler_args.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("-Xexact1".to_string());
            arg
        });
        compiler_args.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--ignored1".to_string());
            arg
        });
        compiler_args.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--ignored2".to_string());
            arg
        });

        create_dummy_manifest(manifest_path.to_str().unwrap(), &compiler_args, &key_pair);

        // SAFETY: ctx is a valid pointer to an ExpectationContext, manifest_path.as_ptr() is a
        // valid pointer to a null-terminated C string.
        let status =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_verify(ctx, manifest_path.as_ptr()) };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);
        // SAFETY: ctx is a valid pointer.
        let error_message = unsafe { AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx) };
        assert!(error_message.is_null());
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_verify_empty_rules_fail_on_non_empty_manifest() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, manifest_path, key_pair) = setup_test_context();

        let mut compiler_args = Vec::new();
        compiler_args.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--foo".to_string());
            arg
        });

        create_dummy_manifest(manifest_path.to_str().unwrap(), &compiler_args, &key_pair);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let status =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_verify(ctx, manifest_path.as_ptr()) };
        assert_eq!(status, FFIVerifierStatus_UNEXPECTED_ARGUMENT);
        // SAFETY: ctx is a valid pointer.
        let error_message = unsafe { AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx) };
        assert_eq!(
            // SAFETY: error_message is guaranteed to be valid if non-null.
            unsafe { CStr::from_ptr(error_message) }.to_str().unwrap(),
            "Unmatched argument in manifest: --foo"
        );
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_verify_invalid_signature() {
        let (ctx, _tmp_pubkey_file, tmp_manifest, manifest_path, key_pair) = setup_test_context();

        // Create a valid manifest first
        let mut compiler_arg = CompilerArgument::new();
        compiler_arg.compiler_flag = Some("-Xexact".to_string());
        let compiler_args = vec![compiler_arg];

        create_dummy_manifest(manifest_path.to_str().unwrap(), &compiler_args, &key_pair);

        let arg = CString::new("-Xexact").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(
                ctx,
                arg.as_ptr(),
                ptr::null_mut(),
                0,
            )
        };

        // Tamper with the manifest file by appending a byte.
        let mut file = std::fs::OpenOptions::new().append(true).open(tmp_manifest.path()).unwrap();
        file.write_all(&[0u8]).unwrap();

        // SAFETY: ctx is a valid pointer.
        let status =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_verify(ctx, manifest_path.as_ptr()) };
        // It might fail parsing (UNABLE_TO_OPEN/FAILURE) or signature check depending on where
        // the corruption hits. Appending usually corrupts protobuf parsing or
        // signature check.
        assert!(status != FFIVerifierStatus_SUCCESS);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_verify_rule_consumption() {
        let (ctx, _tmp_pubkey_file, _tmp_manifest, manifest_path, key_pair) = setup_test_context();

        // Manifest has TWO "--foo" arguments
        let mut compiler_arg = CompilerArgument::new();
        compiler_arg.compiler_flag = Some("--foo".to_string());
        let compiler_args = vec![compiler_arg.clone(), compiler_arg];

        create_dummy_manifest(manifest_path.to_str().unwrap(), &compiler_args, &key_pair);

        // Add ONE ignored rule for "--foo"
        let arg = CString::new("--foo").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(ctx, arg.as_ptr(), false)
        };

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let status =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_verify(ctx, manifest_path.as_ptr()) };

        // Should fail because the second "--foo" has no matching rule (first one consumed it)
        assert_eq!(status, FFIVerifierStatus_UNEXPECTED_ARGUMENT);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };
    }

    #[test]
    fn test_combined_ignored_rule_sequence_match() {
        // 1. Verify Success: Sequence matches ["--ignore1", "--ignore2"]
        let (ctx, _tmp_pubkey_file, _tmp_manifest, manifest_path, key_pair) = setup_test_context();

        let mut compiler_args_success = Vec::new();
        compiler_args_success.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--ignore1".to_string());
            arg
        });
        compiler_args_success.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--ignore2".to_string());
            arg
        });

        create_dummy_manifest(manifest_path.to_str().unwrap(), &compiler_args_success, &key_pair);

        // Create Combined Ignored Rule: ["--ignore1", "--ignore2"]
        let arg1 = CString::new("--ignore1").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(ctx, arg1.as_ptr(), false)
        };
        let arg2 = CString::new("--ignore2").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(
                ctx,
                arg2.as_ptr(),
                false,
            )
        };

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let status =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_verify(ctx, manifest_path.as_ptr()) };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };

        // 2. Verify Failure: Sequence violation ["--ignore1", "--WRONG"]
        let (
            ctx_fail,
            _tmp_pubkey_file_fail,
            _tmp_manifest_fail,
            manifest_path_fail,
            key_pair_fail,
        ) = setup_test_context();

        let mut compiler_args_fail = Vec::new();
        compiler_args_fail.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--ignore1".to_string());
            arg
        });
        compiler_args_fail.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--WRONG".to_string());
            arg
        });

        create_dummy_manifest(
            manifest_path_fail.to_str().unwrap(),
            &compiler_args_fail,
            &key_pair_fail,
        );

        // Create Combined Ignored Rule again: ["--ignore1", "--ignore2"]
        let arg1 = CString::new("--ignore1").unwrap();
        // SAFETY: ctx_fail is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(
                ctx_fail,
                arg1.as_ptr(),
                false,
            )
        };
        let arg2 = CString::new("--ignore2").unwrap();
        // SAFETY: ctx_fail is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(
                ctx_fail,
                arg2.as_ptr(),
                false,
            )
        };

        // SAFETY: ctx_fail is a valid pointer to an ExpectationContext.
        let status_fail = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_fail, manifest_path_fail.as_ptr())
        };
        assert_eq!(status_fail, FFIVerifierStatus_UNEXPECTED_ARGUMENT);
        // SAFETY: ctx_fail is a valid pointer.
        let error_message_fail =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_fail) };
        assert_eq!(
            // SAFETY: error_message_fail is guaranteed to be valid if non-null.
            unsafe { CStr::from_ptr(error_message_fail) }.to_str().unwrap(),
            "Unmatched argument in manifest: --ignore1"
        );
        // SAFETY: ctx_fail is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx_fail) };
    }

    #[test]
    fn test_combined_disallowed_rule_sequence_match() {
        // 1. Verify Success: Sequence ["--bad1", "--good"] does NOT match Disallowed ["--bad1",
        //    "--bad2"]
        // We need subsequent rules to consume the arguments since Disallowed won't.
        let mut compiler_args_success = Vec::new();
        compiler_args_success.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--bad1".to_string());
            arg
        });
        compiler_args_success.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--good".to_string());
            arg
        });

        let (ctx, _tmp_pubkey_file, _tmp_manifest, manifest_path, key_pair) = setup_test_context();

        create_dummy_manifest(manifest_path.to_str().unwrap(), &compiler_args_success, &key_pair);

        // Disallowed Rule: ["--bad1", "--bad2"]
        let arg1 = CString::new("--bad1").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addDisallowedArgumentRule(
                ctx,
                arg1.as_ptr(),
                false,
            )
        };
        let arg2 = CString::new("--bad2").unwrap();
        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(
                ctx,
                arg2.as_ptr(),
                false,
            )
        };

        // Add an Ignored rule to consume the arguments if the Disallowed rule doesn't match.
        let arg1_ignored = CString::new("--bad1").unwrap();
        // SAFETY: ctx is a valid pointer.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(
                ctx,
                arg1_ignored.as_ptr(),
                false,
            )
        };
        let arg2_ignored = CString::new("--good").unwrap();
        // SAFETY: ctx is a valid pointer.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(
                ctx,
                arg2_ignored.as_ptr(),
                false,
            )
        };

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        let status =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_verify(ctx, manifest_path.as_ptr()) };
        assert_eq!(status, FFIVerifierStatus_SUCCESS);

        // SAFETY: ctx is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx) };

        // 2. Verify Failure: Sequence ["--bad1", "--bad2"] MATCHES Disallowed ["--bad1", "--bad2"]
        let mut compiler_args_fail = Vec::new();
        compiler_args_fail.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--bad1".to_string());
            arg
        });
        compiler_args_fail.push({
            let mut arg = CompilerArgument::new();
            arg.compiler_flag = Some("--bad2".to_string());
            arg
        });

        let (
            ctx_fail,
            _tmp_pubkey_file_fail,
            _tmp_manifest_fail,
            manifest_path_fail,
            key_pair_fail,
        ) = setup_test_context();

        create_dummy_manifest(
            manifest_path_fail.to_str().unwrap(),
            &compiler_args_fail,
            &key_pair_fail,
        );

        // Create Combined Disallowed Rule again: ["--bad1", "--bad2"]
        let arg1 = CString::new("--bad1").unwrap();
        // SAFETY: ctx_fail is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_addDisallowedArgumentRule(
                ctx_fail,
                arg1.as_ptr(),
                false,
            )
        };
        let arg2 = CString::new("--bad2").unwrap();
        // SAFETY: ctx_fail is a valid pointer to an ExpectationContext.
        unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(
                ctx_fail,
                arg2.as_ptr(),
                false,
            )
        };

        // SAFETY: ctx_fail is a valid pointer to an ExpectationContext.
        let status_fail = unsafe {
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_fail, manifest_path_fail.as_ptr())
        };
        assert_eq!(status_fail, FFIVerifierStatus_PROHIBITED_ARGUMENT);
        // SAFETY: ctx_fail is a valid pointer.
        let error_message_fail =
            unsafe { AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_fail) };
        assert_eq!(
            // SAFETY: error_message_fail is guaranteed to be valid if non-null.
            unsafe { CStr::from_ptr(error_message_fail) }.to_str().unwrap(),
            "Disallowed argument rule { --bad1 } failed"
        );
        // SAFETY: ctx_fail is a valid pointer to an ExpectationContext.
        unsafe { AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx_fail) };
    }
}
