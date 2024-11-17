// Copyright 2021, The Android Open Source Project
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

//! Wrapper to libselinux

use anyhow::{anyhow, bail, Context, Result};
use std::ffi::{c_int, CStr, CString};
use std::fmt;
use std::io;
use std::ops::Deref;
use std::os::fd::AsRawFd;
use std::os::raw::c_char;
use std::ptr;
use std::sync;

static SELINUX_LOG_INIT: sync::Once = sync::Once::new();

fn redirect_selinux_logs_to_logcat() {
    let cb =
        selinux_bindgen::selinux_callback { func_log: Some(selinux_bindgen::selinux_log_callback) };
    // SAFETY: `selinux_set_callback` assigns the static lifetime function pointer
    // `selinux_log_callback` to a static lifetime variable.
    unsafe {
        selinux_bindgen::selinux_set_callback(selinux_bindgen::SELINUX_CB_LOG as c_int, cb);
    }
}

fn init_logger_once() {
    SELINUX_LOG_INIT.call_once(redirect_selinux_logs_to_logcat)
}

// Partially copied from system/security/keystore2/selinux/src/lib.rs
/// SeContext represents an SELinux context string. It can take ownership of a raw
/// s-string as allocated by `getcon` or `selabel_lookup`. In this case it uses
/// `freecon` to free the resources when dropped. In its second variant it stores
/// an `std::ffi::CString` that can be initialized from a Rust string slice.
#[derive(Debug)]
#[allow(dead_code)] // CString variant is used in tests
pub enum SeContext {
    /// Wraps a raw context c-string as returned by libselinux.
    Raw(*mut ::std::os::raw::c_char),
    /// Stores a context string as `std::ffi::CString`.
    CString(CString),
}

impl PartialEq for SeContext {
    fn eq(&self, other: &Self) -> bool {
        // We dereference both and thereby delegate the comparison
        // to `CStr`'s implementation of `PartialEq`.
        **self == **other
    }
}

impl Eq for SeContext {}

impl fmt::Display for SeContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_str().unwrap_or("Invalid context"))
    }
}

impl Drop for SeContext {
    fn drop(&mut self) {
        if let Self::Raw(p) = self {
            // SAFETY: SeContext::Raw is created only with a pointer that is set by libselinux and
            // has to be freed with freecon.
            unsafe { selinux_bindgen::freecon(*p) };
        }
    }
}

impl Deref for SeContext {
    type Target = CStr;

    fn deref(&self) -> &Self::Target {
        match self {
            // SAFETY: the non-owned C string pointed by `p` is guaranteed to be valid (non-null
            // and shorter than i32::MAX). It is freed when SeContext is dropped.
            Self::Raw(p) => unsafe { CStr::from_ptr(*p) },
            Self::CString(cstr) => cstr,
        }
    }
}

impl SeContext {
    /// Initializes the `SeContext::CString` variant from a Rust string slice.
    #[allow(dead_code)] // Used in tests
    pub fn new(con: &str) -> Result<Self> {
        Ok(Self::CString(
            CString::new(con)
                .with_context(|| format!("Failed to create SeContext with \"{}\"", con))?,
        ))
    }

    pub fn selinux_type(&self) -> Result<&str> {
        let context = self.deref().to_str().context("Label is not valid UTF8")?;

        // The syntax is user:role:type:sensitivity[:category,...],
        // ignoring security level ranges, which don't occur on Android. See
        // https://github.com/SELinuxProject/selinux-notebook/blob/main/src/security_context.md
        // We only want the type.
        let fields: Vec<_> = context.split(':').collect();
        if fields.len() < 4 || fields.len() > 5 {
            bail!("Syntactically invalid label {}", self);
        }
        Ok(fields[2])
    }
}

/// Takes ownership of context handle returned by `selinux_android_tee_service_context_handle`
/// and closes it via `selabel_close` when dropped.
struct TeeServiceSelinuxBackend {
    handle: *mut selinux_bindgen::selabel_handle,
}

impl TeeServiceSelinuxBackend {
    const BACKEND_ID: i32 = selinux_bindgen::SELABEL_CTX_ANDROID_SERVICE as i32;

    /// Creates a new instance representing selinux context handle returned from
    /// `selinux_android_tee_service_context_handle`.
    fn new() -> Result<Self> {
        // SAFETY: selinux_android_tee_service_context_handle is always safe to call. The returned
        // handle is valid until `selabel_close` is called on it (see the safety comment on the drop
        // trait).
        let handle = unsafe { selinux_bindgen::selinux_android_tee_service_context_handle() };
        if handle.is_null() {
            Err(anyhow!("selinux_android_tee_service_context_handle returned a NULL context"))
        } else {
            Ok(TeeServiceSelinuxBackend { handle })
        }
    }

    fn lookup(&self, tee_service: &str) -> Result<SeContext> {
        let mut con: *mut c_char = ptr::null_mut();
        let c_key = CString::new(tee_service).context("failed to convert to CString")?;
        // SAFETY: the returned pointer `con` is valid until `freecon` is called on it.
        match unsafe {
            selinux_bindgen::selabel_lookup(self.handle, &mut con, c_key.as_ptr(), Self::BACKEND_ID)
        } {
            0 => {
                if !con.is_null() {
                    Ok(SeContext::Raw(con))
                } else {
                    Err(anyhow!("selabel_lookup returned a NULL context"))
                }
            }
            _ => Err(anyhow!(io::Error::last_os_error())).context("selabel_lookup failed"),
        }
    }
}

impl Drop for TeeServiceSelinuxBackend {
    fn drop(&mut self) {
        // SAFETY: the TeeServiceSelinuxBackend is created only with a pointer is set by
        // libselinux and has to be freed with `selabel_close`.
        unsafe { selinux_bindgen::selabel_close(self.handle) };
    }
}

pub fn getfilecon<F: AsRawFd>(file: &F) -> Result<SeContext> {
    let fd = file.as_raw_fd();
    let mut con: *mut c_char = ptr::null_mut();
    // SAFETY: the returned pointer `con` is wrapped in SeContext::Raw which is freed with
    // `freecon` when it is dropped.
    match unsafe { selinux_bindgen::fgetfilecon(fd, &mut con) } {
        1.. => {
            if !con.is_null() {
                Ok(SeContext::Raw(con))
            } else {
                Err(anyhow!("fgetfilecon returned a NULL context"))
            }
        }
        _ => Err(anyhow!(io::Error::last_os_error())).context("fgetfilecon failed"),
    }
}

pub fn getprevcon() -> Result<SeContext> {
    let mut con: *mut c_char = ptr::null_mut();
    // SAFETY: the returned pointer `con` is wrapped in SeContext::Raw which is freed with
    // `freecon` when it is dropped.
    match unsafe { selinux_bindgen::getprevcon(&mut con) } {
        0.. => {
            if !con.is_null() {
                Ok(SeContext::Raw(con))
            } else {
                Err(anyhow!("getprevcon returned a NULL context"))
            }
        }
        _ => Err(anyhow!(io::Error::last_os_error())).context("getprevcon failed"),
    }
}

// Wrapper around selinux_check_access
fn check_access(source: &CStr, target: &CStr, tclass: &str, perm: &str) -> Result<()> {
    let c_tclass = CString::new(tclass).context("failed to convert tclass to CString")?;
    let c_perm = CString::new(perm).context("failed to convert perm to CString")?;

    // SAFETY: lifecycle of pointers passed to the selinux_check_access outlive the duration of the
    // call.
    match unsafe {
        selinux_bindgen::selinux_check_access(
            source.as_ptr(),
            target.as_ptr(),
            c_tclass.as_ptr(),
            c_perm.as_ptr(),
            ptr::null_mut(),
        )
    } {
        0 => Ok(()),
        _ => Err(anyhow!(io::Error::last_os_error())).with_context(|| {
            format!(
                "check_access: Failed with sctx: {:?} tctx: {:?} tclass: {:?} perm {:?}",
                source, target, tclass, perm
            )
        }),
    }
}

pub fn check_tee_service_permission(caller_ctx: &SeContext, tee_services: &[String]) -> Result<()> {
    init_logger_once();

    let backend = TeeServiceSelinuxBackend::new()?;

    for tee_service in tee_services {
        let tee_service_ctx = backend.lookup(tee_service)?;
        check_access(caller_ctx, &tee_service_ctx, "tee_service", "use")
            .with_context(|| format!("permission denied for {:?}", tee_service))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "disabling test while investigating b/379087641"]
    fn test_check_tee_service_permission_has_permission() -> Result<()> {
        if cfg!(not(tee_services_allowlist)) {
            // Skip test on release configurations without tee_services_allowlist feature enabled.
            return Ok(());
        }

        let caller_ctx = SeContext::new("u:r:shell:s0")?;
        let tee_services = [String::from("test_pkvm_tee_service")];
        check_tee_service_permission(&caller_ctx, &tee_services)
    }

    #[test]
    #[ignore = "disabling test while investigating b/379087641"]
    fn test_check_tee_service_permission_invalid_tee_service() -> Result<()> {
        if cfg!(not(tee_services_allowlist)) {
            // Skip test on release configurations without tee_services_allowlist feature enabled.
            return Ok(());
        }

        let caller_ctx = SeContext::new("u:r:shell:s0")?;
        let tee_services = [String::from("test_tee_service_does_not_exist")];
        let ret = check_tee_service_permission(&caller_ctx, &tee_services);
        assert!(ret.is_err());
        Ok(())
    }
}
