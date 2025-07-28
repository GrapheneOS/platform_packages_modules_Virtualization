/*
 * Copyright (C) 2021 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use anyhow::{bail, Result};
use libc::getxattr;
use std::ffi::CString;
use std::io;
use std::os::unix::io::{AsRawFd, BorrowedFd};

const SHA256_HASH_SIZE: usize = 32;

/// Bytes of SHA256 digest
type Sha256Digest = [u8; SHA256_HASH_SIZE];

#[cfg_attr(test, mockall::automock, allow(dead_code))]
pub mod wrapper {
    use super::Sha256Digest;
    use anyhow::Result;
    use std::os::unix::io::BorrowedFd;
    /// Returns the fs-verity measurement/digest. Currently only SHA256 is supported.
    #[cfg(test)]
    #[allow(clippy::needless_lifetimes)]
    pub fn measure<'a>(fd: BorrowedFd<'a>) -> Result<Sha256Digest> {
        super::measure(fd)
    }

    #[cfg(not(test))]
    pub fn measure(fd: BorrowedFd) -> Result<Sha256Digest> {
        super::measure(fd)
    }
}

fn measure(fd: BorrowedFd) -> Result<Sha256Digest> {
    // TODO(b/196635431): Unfortunately, the FUSE API doesn't allow authfs to implement the
    // standard fs-verity ioctls. Until the kernel allows, use the alternative xattr
    // that authfs provides.
    let path = CString::new(format!("/proc/self/fd/{}", fd.as_raw_fd()).as_str()).unwrap();
    let name = CString::new("authfs.fsverity.digest").unwrap();
    let mut buf = [0u8; SHA256_HASH_SIZE];
    // SAFETY: should not write past end of buffer.
    let size = unsafe {
        getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr() as *mut libc::c_void, buf.len())
    };
    if size < 0 {
        bail!("Failed to getxattr: {}", io::Error::last_os_error());
    } else if size != SHA256_HASH_SIZE as isize {
        bail!("Unexpected hash size: {}", size);
    } else {
        Ok(buf)
    }
}
