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
use std::ffi::{CStr, CString};
use std::fmt;
use std::io;
use std::ops::Deref;
use std::os::fd::AsRawFd;
use std::os::raw::c_char;
use std::ptr;

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
