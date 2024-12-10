// Copyright 2024 The Android Open Source Project
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

//! Stable C library for AVF.

use std::ffi::CStr;
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::raw::{c_char, c_int};
use std::ptr;

use android_system_virtualizationservice::{
    aidl::android::system::virtualizationservice::{
        DiskImage::DiskImage, IVirtualizationService::IVirtualizationService,
        VirtualMachineConfig::VirtualMachineConfig,
        VirtualMachineRawConfig::VirtualMachineRawConfig,
    },
    binder::{ParcelFileDescriptor, Strong},
};
use avf_bindgen::StopReason;
use vmclient::{DeathReason, VirtualizationService, VmInstance};

/// Create a new virtual machine config object with no properties.
#[no_mangle]
pub extern "C" fn AVirtualMachineRawConfig_create() -> *mut VirtualMachineRawConfig {
    let config = Box::new(VirtualMachineRawConfig {
        platformVersion: "~1.0".to_owned(),
        ..Default::default()
    });
    Box::into_raw(config)
}

/// Destroy a virtual machine config object.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`. `config` must not be
/// used after deletion.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_destroy(config: *mut VirtualMachineRawConfig) {
    if !config.is_null() {
        // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
        // AVirtualMachineRawConfig_create. It's the only reference to the object.
        unsafe {
            let _ = Box::from_raw(config);
        }
    }
}

/// Set a name of a virtual machine.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setName(
    config: *mut VirtualMachineRawConfig,
    name: *const c_char,
) -> c_int {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    // SAFETY: `name` is assumed to be a pointer to a valid C string.
    config.name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
    0
}

/// Set an instance ID of a virtual machine.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`. `instanceId` must be a
/// valid, non-null pointer to 64-byte data.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setInstanceId(
    config: *mut VirtualMachineRawConfig,
    instance_id: *const u8,
) -> c_int {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    // SAFETY: `instanceId` is assumed to be a valid pointer to 64 bytes of memory. `config`
    // is assumed to be a valid object returned by AVirtuaMachineConfig_create.
    // Both never overlap.
    unsafe {
        ptr::copy_nonoverlapping(instance_id, config.instanceId.as_mut_ptr(), 64);
    }
    0
}

/// Set a kernel image of a virtual machine.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`. `fd` must be a valid
/// file descriptor or -1. `AVirtualMachineRawConfig_setKernel` takes ownership of `fd` and `fd`
/// will be closed upon `AVirtualMachineRawConfig_delete`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setKernel(
    config: *mut VirtualMachineRawConfig,
    fd: c_int,
) -> c_int {
    let file = get_file_from_fd(fd);
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.kernel = file.map(ParcelFileDescriptor::new);
    0
}

/// Set an init rd of a virtual machine.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`. `fd` must be a valid
/// file descriptor or -1. `AVirtualMachineRawConfig_setInitRd` takes ownership of `fd` and `fd`
/// will be closed upon `AVirtualMachineRawConfig_delete`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setInitRd(
    config: *mut VirtualMachineRawConfig,
    fd: c_int,
) -> c_int {
    let file = get_file_from_fd(fd);
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.initrd = file.map(ParcelFileDescriptor::new);
    0
}

/// Add a disk for a virtual machine.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`. `fd` must be a valid
/// file descriptor. `AVirtualMachineRawConfig_addDisk` takes ownership of `fd` and `fd` will be
/// closed upon `AVirtualMachineRawConfig_delete`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_addDisk(
    config: *mut VirtualMachineRawConfig,
    fd: c_int,
    writable: bool,
) -> c_int {
    let file = get_file_from_fd(fd);
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    match file {
        // partition not supported yet
        None => -libc::EINVAL,
        Some(file) => {
            config.disks.push(DiskImage {
                image: Some(ParcelFileDescriptor::new(file)),
                writable,
                ..Default::default()
            });
            0
        }
    }
}

/// Set how much memory will be given to a virtual machine.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setMemoryMib(
    config: *mut VirtualMachineRawConfig,
    memory_mib: i32,
) -> c_int {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.memoryMib = memory_mib;
    0
}

/// Set whether a virtual machine is protected or not.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setProtectedVm(
    config: *mut VirtualMachineRawConfig,
    protected_vm: bool,
) -> c_int {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.protectedVm = protected_vm;
    0
}

/// Set whether a virtual machine uses memory ballooning or not.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setBalloon(
    config: *mut VirtualMachineRawConfig,
    balloon: bool,
) -> c_int {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.noBalloon = !balloon;
    0
}

/// NOT IMPLEMENTED.
///
/// # Returns
/// It always returns `-ENOTSUP`.
#[no_mangle]
pub extern "C" fn AVirtualMachineRawConfig_setHypervisorSpecificAuthMethod(
    _config: *mut VirtualMachineRawConfig,
    _enable: bool,
) -> c_int {
    -libc::ENOTSUP
}

/// NOT IMPLEMENTED.
///
/// # Returns
/// It always returns `-ENOTSUP`.
#[no_mangle]
pub extern "C" fn AVirtualMachineRawConfig_addCustomMemoryBackingFile(
    _config: *mut VirtualMachineRawConfig,
    _fd: c_int,
    _range_start: usize,
    _range_end: usize,
) -> c_int {
    -libc::ENOTSUP
}

/// Spawn a new instance of `virtmgr`, a child process that will host the `VirtualizationService`
/// AIDL service, and connect to the child process.
///
/// # Safety
/// `service_ptr` must be a valid, non-null pointer to a mutable raw pointer.
#[no_mangle]
pub unsafe extern "C" fn AVirtualizationService_create(
    service_ptr: *mut *mut Strong<dyn IVirtualizationService>,
    early: bool,
) -> c_int {
    let virtmgr =
        if early { VirtualizationService::new_early() } else { VirtualizationService::new() };
    let virtmgr = match virtmgr {
        Ok(virtmgr) => virtmgr,
        Err(e) => return -e.raw_os_error().unwrap_or(libc::EIO),
    };
    match virtmgr.connect() {
        Ok(service) => {
            // SAFETY: `service` is assumed to be a valid, non-null pointer to a mutable raw
            // pointer. `service` is the only reference here and `config` takes
            // ownership.
            unsafe {
                *service_ptr = Box::into_raw(Box::new(service));
            }
            0
        }
        Err(_) => -libc::ECONNREFUSED,
    }
}

/// Destroy a VirtualizationService object.
///
/// # Safety
/// `service` must be a pointer returned by `AVirtualizationService_create` or
/// `AVirtualizationService_create_early`. `service` must not be reused after deletion.
#[no_mangle]
pub unsafe extern "C" fn AVirtualizationService_destroy(
    service: *mut Strong<dyn IVirtualizationService>,
) {
    if !service.is_null() {
        // SAFETY: `service` is assumed to be a valid, non-null pointer returned by
        // `AVirtualizationService_create`. It's the only reference to the object.
        unsafe {
            let _ = Box::from_raw(service);
        }
    }
}

/// Create a virtual machine with given `config`.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`. `service` must be a
/// pointer returned by `AVirtualMachineRawConfig_create`. `vm_ptr` must be a valid, non-null
/// pointer to a mutable raw pointer. `console_out_fd`, `console_in_fd`, and `log_fd` must be a
/// valid file descriptor or -1. `AVirtualMachine_create` takes ownership of `console_out_fd`,
/// `console_in_fd`, and `log_fd`, and taken file descriptors must not be reused.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachine_createRaw(
    service: *const Strong<dyn IVirtualizationService>,
    config: *mut VirtualMachineRawConfig,
    console_out_fd: c_int,
    console_in_fd: c_int,
    log_fd: c_int,
    vm_ptr: *mut *mut VmInstance,
) -> c_int {
    // SAFETY: `service` is assumed to be a valid, non-null pointer returned by
    // `AVirtualizationService_create` or `AVirtualizationService_create_early`. It's the only
    // reference to the object.
    let service = unsafe { &*service };

    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // `AVirtualMachineRawConfig_create`. It's the only reference to the object.
    let config = unsafe { *Box::from_raw(config) };
    let config = VirtualMachineConfig::RawConfig(config);

    let console_out = get_file_from_fd(console_out_fd);
    let console_in = get_file_from_fd(console_in_fd);
    let log = get_file_from_fd(log_fd);

    match VmInstance::create(service.as_ref(), &config, console_out, console_in, log, None, None) {
        Ok(vm) => {
            // SAFETY: `vm_ptr` is assumed to be a valid, non-null pointer to a mutable raw pointer.
            // `vm` is the only reference here and `vm_ptr` takes ownership.
            unsafe {
                *vm_ptr = Box::into_raw(Box::new(vm));
            }
            0
        }
        Err(_) => -libc::EIO,
    }
}

/// Start a virtual machine.
///
/// # Safety
/// `vm` must be a pointer returned by `AVirtualMachine_createRaw`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachine_start(vm: *const VmInstance) -> c_int {
    // SAFETY: `vm` is assumed to be a valid, non-null pointer returned by
    // `AVirtualMachine_createRaw`. It's the only reference to the object.
    let vm = unsafe { &*vm };
    match vm.start() {
        Ok(_) => 0,
        Err(_) => -libc::EIO,
    }
}

/// Stop a virtual machine.
///
/// # Safety
/// `vm` must be a pointer returned by `AVirtualMachine_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachine_stop(vm: *const VmInstance) -> c_int {
    // SAFETY: `vm` is assumed to be a valid, non-null pointer returned by
    // `AVirtualMachine_createRaw`. It's the only reference to the object.
    let vm = unsafe { &*vm };
    match vm.stop() {
        Ok(_) => 0,
        Err(_) => -libc::EIO,
    }
}

/// Wait until a virtual machine stops.
///
/// # Safety
/// `vm` must be a pointer returned by `AVirtualMachine_createRaw`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachine_waitForStop(vm: *const VmInstance) -> StopReason {
    // SAFETY: `vm` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachine_create. It's the only reference to the object.
    let vm = unsafe { &*vm };
    match vm.wait_for_death() {
        DeathReason::VirtualizationServiceDied => StopReason::VIRTUALIZATION_SERVICE_DIED,
        DeathReason::InfrastructureError => StopReason::INFRASTRUCTURE_ERROR,
        DeathReason::Killed => StopReason::KILLED,
        DeathReason::Unknown => StopReason::UNKNOWN,
        DeathReason::Shutdown => StopReason::SHUTDOWN,
        DeathReason::StartFailed => StopReason::START_FAILED,
        DeathReason::Reboot => StopReason::REBOOT,
        DeathReason::Crash => StopReason::CRASH,
        DeathReason::PvmFirmwarePublicKeyMismatch => StopReason::PVM_FIRMWARE_PUBLIC_KEY_MISMATCH,
        DeathReason::PvmFirmwareInstanceImageChanged => {
            StopReason::PVM_FIRMWARE_INSTANCE_IMAGE_CHANGED
        }
        DeathReason::Hangup => StopReason::HANGUP,
        _ => StopReason::UNRECOGNISED,
    }
}

/// Destroy a virtual machine.
///
/// # Safety
/// `vm` must be a pointer returned by `AVirtualMachine_createRaw`. `vm` must not be reused after
/// deletion.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachine_destroy(vm: *mut VmInstance) {
    if !vm.is_null() {
        // SAFETY: `vm` is assumed to be a valid, non-null pointer returned by
        // AVirtualMachine_create. It's the only reference to the object.
        unsafe {
            let _ = Box::from_raw(vm);
        }
    }
}

fn get_file_from_fd(fd: i32) -> Option<File> {
    if fd == -1 {
        None
    } else {
        // SAFETY: transferring ownership of `fd` from the caller
        Some(unsafe { File::from_raw_fd(fd) })
    }
}
