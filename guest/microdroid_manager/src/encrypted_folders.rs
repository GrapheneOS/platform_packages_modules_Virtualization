// Copyright 2026, The Android Open Source Project
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

// Functions to manage per-folder encryption using fs-crypt

use fs_crypt_bindgen::{
    fscrypt_add_key_arg, fscrypt_policy_v2, FSCRYPT_KEY_IDENTIFIER_SIZE,
    FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, FSCRYPT_MAX_KEY_SIZE, FSCRYPT_MODE_AES_256_CTS,
    FSCRYPT_MODE_AES_256_XTS, FSCRYPT_POLICY_FLAGS_PAD_32,
};
use log::info;
use rustix::fd::{AsRawFd, OwnedFd};
use rustix::fs::{open, Mode, OFlags};
use std::path::Path;
use zerocopy::*;

// Take a key and add it to the filesystem specified by fd
// Returns an identifier that can be used to set policy on a folder
fn add_encryption_key(
    key: &[u8; FSCRYPT_MAX_KEY_SIZE as usize],
    fd: &OwnedFd,
) -> std::io::Result<[u8; FSCRYPT_KEY_IDENTIFIER_SIZE as usize]> {
    const FS_IOC_ADD_ENCRYPTION_KEY: libc::c_int = 0xc0506617u32 as i32;

    let mut key_arg: Box<fscrypt_add_key_arg<[u8]>> =
        FromZeros::new_box_zeroed_with_elems(key.len()).unwrap();
    key_arg.key_spec.type_ = FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER;
    key_arg.raw_size = key.len() as u32;
    key_arg.raw.copy_from_slice(key);
    let key_arg_ptr: *const fscrypt_add_key_arg = &*key_arg as *const _ as _;

    // SAFETY: Use correct struct for ioctl. Also safe to read identifier
    // since we set raw_size to the dynamic size of the trailing array.
    unsafe {
        match libc::ioctl(fd.as_raw_fd(), FS_IOC_ADD_ENCRYPTION_KEY, key_arg_ptr) {
            0 => Ok(key_arg.key_spec.u.identifier),
            _ => Err(std::io::Error::last_os_error()),
        }
    }
}

// Set encryption policy on a folder. Takes a key_identifier returned from add_encryption_key
// Succeeds if either the folder is empty, or the policy exists and the key matches the one given
fn set_encryption_policy(
    key_identifier: &[u8; FSCRYPT_KEY_IDENTIFIER_SIZE as usize],
    fd: &OwnedFd,
) -> std::io::Result<()> {
    const FS_IOC_SET_ENCRYPTION_POLICY: libc::c_int = 0x800c6613u32 as i32;

    let policy = fscrypt_policy_v2 {
        version: 2,
        contents_encryption_mode: FSCRYPT_MODE_AES_256_XTS as u8,
        filenames_encryption_mode: FSCRYPT_MODE_AES_256_CTS as u8,
        flags: FSCRYPT_POLICY_FLAGS_PAD_32 as u8,
        log2_data_unit_size: 0,
        __reserved: [0; 3],
        master_key_identifier: *key_identifier,
    };

    // SAFETY: Using correct struct for ioctl
    unsafe {
        match libc::ioctl(fd.as_raw_fd(), FS_IOC_SET_ENCRYPTION_POLICY, &policy) {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }
}

// Takes a key, installs it and sets it on a path.
// Fails if the key can't be installed.
// Succeeds if either the folder is empty, or is already encrypted with the same key
pub fn set_encryption_key(file_path: &Path, key: &[u8]) -> std::io::Result<()> {
    if key.len() > FSCRYPT_MAX_KEY_SIZE as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Key size ({}) exceeds maximum allowed size ({})",
                key.len(),
                FSCRYPT_MAX_KEY_SIZE
            ),
        ));
    }

    let file = open(file_path, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty())?;

    let mut secret_key = [0u8; FSCRYPT_MAX_KEY_SIZE as usize];
    secret_key[..key.len()].copy_from_slice(key);

    let key_identifier = add_encryption_key(&secret_key, &file)?;
    set_encryption_policy(&key_identifier, &file)?;

    info!("Successfully enabled fs-crypt for: {}", file_path.display());
    Ok(())
}
