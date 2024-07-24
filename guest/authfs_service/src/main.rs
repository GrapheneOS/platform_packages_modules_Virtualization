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

//! AuthFsService facilitates authfs mounting (which is a privileged operation) for the client. The
//! client will provide an `AuthFsConfig` which includes the backend address (only port for now) and
//! the filesystem configuration. It is up to the client to ensure the backend server is running. On
//! a successful mount, the client receives an `IAuthFs`, and through the binder object, the client
//! is able to retrieve "remote file descriptors".

mod authfs;

use anyhow::{bail, Result};
use log::*;
use rpcbinder::RpcServer;
use rustutils::sockets::android_get_control_socket;
use std::ffi::OsString;
use std::fs::{create_dir, read_dir, remove_dir_all, remove_file};
use std::os::unix::io::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicUsize, Ordering};

use authfs_aidl_interface::aidl::com::android::virt::fs::AuthFsConfig::AuthFsConfig;
use authfs_aidl_interface::aidl::com::android::virt::fs::IAuthFs::IAuthFs;
use authfs_aidl_interface::aidl::com::android::virt::fs::IAuthFsService::{
    BnAuthFsService, IAuthFsService, AUTHFS_SERVICE_SOCKET_NAME,
};
use binder::{self, BinderFeatures, ExceptionCode, Interface, Status, Strong};

const SERVICE_ROOT: &str = "/data/misc/authfs";

/// Implementation of `IAuthFsService`.
pub struct AuthFsService {
    serial_number: AtomicUsize,
    debuggable: bool,
}

impl Interface for AuthFsService {}

impl IAuthFsService for AuthFsService {
    fn mount(&self, config: &AuthFsConfig) -> binder::Result<Strong<dyn IAuthFs>> {
        self.validate(config)?;

        let mountpoint = self.get_next_mount_point();

        // The directory is supposed to be deleted when `AuthFs` is dropped.
        create_dir(&mountpoint).map_err(|e| {
            Status::new_service_specific_error_str(
                -1,
                Some(format!("Cannot create mount directory {:?}: {:?}", &mountpoint, e)),
            )
        })?;

        authfs::AuthFs::mount_and_wait(mountpoint, config, self.debuggable).map_err(|e| {
            Status::new_service_specific_error_str(
                -1,
                Some(format!("mount_and_wait failed: {:?}", e)),
            )
        })
    }
}

impl AuthFsService {
    fn new_binder(debuggable: bool) -> Strong<dyn IAuthFsService> {
        let service = AuthFsService { serial_number: AtomicUsize::new(1), debuggable };
        BnAuthFsService::new_binder(service, BinderFeatures::default())
    }

    fn validate(&self, config: &AuthFsConfig) -> binder::Result<()> {
        if config.port < 0 {
            return Err(Status::new_exception_str(
                ExceptionCode::ILLEGAL_ARGUMENT,
                Some(format!("Invalid port: {}", config.port)),
            ));
        }
        Ok(())
    }

    fn get_next_mount_point(&self) -> OsString {
        let previous = self.serial_number.fetch_add(1, Ordering::Relaxed);
        OsString::from(format!("{}/{}", SERVICE_ROOT, previous))
    }
}

fn clean_up_working_directory() -> Result<()> {
    for entry in read_dir(SERVICE_ROOT)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            remove_dir_all(path)?;
        } else if path.is_file() {
            remove_file(path)?;
        } else {
            bail!("Unrecognized path type: {:?}", path);
        }
    }
    Ok(())
}

/// Prepares a socket file descriptor for the authfs service.
///
/// # Safety requirement
///
/// The caller must ensure that this function is the only place that claims ownership
/// of the file descriptor and it is called only once.
unsafe fn prepare_authfs_service_socket() -> Result<OwnedFd> {
    let raw_fd = android_get_control_socket(AUTHFS_SERVICE_SOCKET_NAME)?;

    // Creating OwnedFd for stdio FDs is not safe.
    if [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO].contains(&raw_fd) {
        bail!("File descriptor {raw_fd} is standard I/O descriptor");
    }
    // SAFETY: Initializing OwnedFd for a RawFd created by the init.
    // We checked that the integer value corresponds to a valid FD and that the caller
    // ensures that this is the only place to claim its ownership.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

#[allow(clippy::eq_op)]
fn try_main() -> Result<()> {
    let debuggable = env!("TARGET_BUILD_VARIANT") != "user";
    let log_level = if debuggable { log::LevelFilter::Trace } else { log::LevelFilter::Info };
    android_logger::init_once(
        android_logger::Config::default().with_tag("authfs_service").with_max_level(log_level),
    );

    clean_up_working_directory()?;

    // SAFETY: This is the only place we take the ownership of the fd of the authfs service.
    let socket_fd = unsafe { prepare_authfs_service_socket()? };
    let service = AuthFsService::new_binder(debuggable).as_binder();
    debug!("{} is starting as a rpc service.", AUTHFS_SERVICE_SOCKET_NAME);
    let server = RpcServer::new_bound_socket(service, socket_fd)?;
    info!("The RPC server '{}' is running.", AUTHFS_SERVICE_SOCKET_NAME);
    server.join();
    info!("The RPC server at '{}' has shut down gracefully.", AUTHFS_SERVICE_SOCKET_NAME);
    Ok(())
}

fn main() {
    if let Err(e) = try_main() {
        error!("failed with {:?}", e);
        std::process::exit(1);
    }
}
