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
use std::os::fd::{FromRawFd, IntoRawFd};
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::time::Duration;

use android_system_virtualizationservice::{
    aidl::android::system::virtualizationservice::{
        AssignedDevices::AssignedDevices, CpuOptions::CpuOptions,
        CpuOptions::CpuTopology::CpuTopology, CustomMemoryBackingFile::CustomMemoryBackingFile,
        DiskImage::DiskImage, IVirtualizationService::IVirtualizationService,
        VirtualMachineConfig::VirtualMachineConfig,
        VirtualMachineRawConfig::VirtualMachineRawConfig,
    },
    binder::{ParcelFileDescriptor, Strong},
};
use avf_bindgen::AVirtualMachineStopReason;
use libc::timespec;
use log::error;
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
    let name = unsafe { CStr::from_ptr(name) };
    match name.to_str() {
        Ok(name) => {
            config.name = name.to_owned();
            0
        }
        Err(_) => -libc::EINVAL,
    }
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
    instance_id_size: usize,
) -> c_int {
    if instance_id_size != 64 {
        return -libc::EINVAL;
    }

    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    // SAFETY: `instanceId` is assumed to be a valid pointer to 64 bytes of memory. `config`
    // is assumed to be a valid object returned by AVirtuaMachineConfig_create.
    // Both never overlap.
    unsafe {
        ptr::copy_nonoverlapping(instance_id, config.instanceId.as_mut_ptr(), instance_id_size);
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
) {
    let file = get_file_from_fd(fd);
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.kernel = file.map(ParcelFileDescriptor::new);
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
) {
    let file = get_file_from_fd(fd);
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.initrd = file.map(ParcelFileDescriptor::new);
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
pub unsafe extern "C" fn AVirtualMachineRawConfig_setMemoryMiB(
    config: *mut VirtualMachineRawConfig,
    memory_mib: i32,
) {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.memoryMib = memory_mib;
}

/// Set how much swiotlb will be given to a virtual machine.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setSwiotlbMiB(
    config: *mut VirtualMachineRawConfig,
    swiotlb_mib: i32,
) {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.swiotlbMib = swiotlb_mib;
}

/// Set vCPU count.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setVCpuCount(
    config: *mut VirtualMachineRawConfig,
    n: i32,
) {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.cpuOptions = CpuOptions { cpuTopology: CpuTopology::CpuCount(n) };
}

/// Set whether a virtual machine is protected or not.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setProtectedVm(
    config: *mut VirtualMachineRawConfig,
    protected_vm: bool,
) {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.protectedVm = protected_vm;
}

/// Set whether to use an alternate, hypervisor-specific authentication method for protected VMs.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setHypervisorSpecificAuthMethod(
    config: *mut VirtualMachineRawConfig,
    enable: bool,
) -> c_int {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };
    config.enableHypervisorSpecificAuthMethod = enable;
    // We don't validate whether this is supported until later, when the VM is started.
    0
}

/// Use the specified fd as the backing memfd for a range of the guest physical memory.
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_addCustomMemoryBackingFile(
    config: *mut VirtualMachineRawConfig,
    fd: c_int,
    range_start: u64,
    range_end: u64,
) -> c_int {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };

    let Some(file) = get_file_from_fd(fd) else {
        return -libc::EINVAL;
    };
    let Some(size) = range_end.checked_sub(range_start) else {
        return -libc::EINVAL;
    };
    config.customMemoryBackingFiles.push(CustomMemoryBackingFile {
        file: Some(ParcelFileDescriptor::new(file)),
        // AIDL doesn't support unsigned ints, so we've got to reinterpret the bytes into a signed
        // int.
        rangeStart: range_start as i64,
        size: size as i64,
    });
    0
}

/// Add device tree overlay blob
///
/// # Safety
/// `config` must be a pointer returned by `AVirtualMachineRawConfig_create`. `fd` must be a valid
/// file descriptor or -1. `AVirtualMachineRawConfig_setDeviceTreeOverlay` takes ownership of `fd`
/// and `fd` will be closed upon `AVirtualMachineRawConfig_delete`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachineRawConfig_setDeviceTreeOverlay(
    config: *mut VirtualMachineRawConfig,
    fd: c_int,
) {
    // SAFETY: `config` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachineRawConfig_create. It's the only reference to the object.
    let config = unsafe { &mut *config };

    match get_file_from_fd(fd) {
        Some(file) => {
            let fd = ParcelFileDescriptor::new(file);
            config.devices = AssignedDevices::Dtbo(Some(fd));
        }
        _ => {
            config.devices = Default::default();
        }
    };
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

    match VmInstance::create(service.as_ref(), &config, console_out, console_in, log, None) {
        Ok(vm) => {
            // SAFETY: `vm_ptr` is assumed to be a valid, non-null pointer to a mutable raw pointer.
            // `vm` is the only reference here and `vm_ptr` takes ownership.
            unsafe {
                *vm_ptr = Box::into_raw(Box::new(vm));
            }
            0
        }
        Err(e) => {
            error!("AVirtualMachine_createRaw failed: {e:?}");
            -libc::EIO
        }
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
    match vm.start(None) {
        Ok(_) => 0,
        Err(e) => {
            error!("AVirtualMachine_start failed: {e:?}");
            -libc::EIO
        }
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
        Err(e) => {
            error!("AVirtualMachine_stop failed: {e:?}");
            -libc::EIO
        }
    }
}

/// Open a vsock connection to the CID of the virtual machine on the given vsock port.
///
/// # Safety
/// `vm` must be a pointer returned by `AVirtualMachine_create`.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachine_connectVsock(vm: *const VmInstance, port: u32) -> c_int {
    // SAFETY: `vm` is assumed to be a valid, non-null pointer returned by
    // `AVirtualMachine_createRaw`. It's the only reference to the object.
    let vm = unsafe { &*vm };
    match vm.connect_vsock(port) {
        Ok(pfd) => pfd.into_raw_fd(),
        Err(e) => {
            error!("AVirtualMachine_connectVsock failed: {e:?}");
            -libc::EIO
        }
    }
}

fn death_reason_to_stop_reason(death_reason: DeathReason) -> AVirtualMachineStopReason {
    match death_reason {
        DeathReason::VirtualizationServiceDied => {
            AVirtualMachineStopReason::AVIRTUAL_MACHINE_VIRTUALIZATION_SERVICE_DIED
        }
        DeathReason::InfrastructureError => {
            AVirtualMachineStopReason::AVIRTUAL_MACHINE_INFRASTRUCTURE_ERROR
        }
        DeathReason::Killed => AVirtualMachineStopReason::AVIRTUAL_MACHINE_KILLED,
        DeathReason::Unknown => AVirtualMachineStopReason::AVIRTUAL_MACHINE_UNKNOWN,
        DeathReason::Shutdown => AVirtualMachineStopReason::AVIRTUAL_MACHINE_SHUTDOWN,
        DeathReason::StartFailed => AVirtualMachineStopReason::AVIRTUAL_MACHINE_START_FAILED,
        DeathReason::Reboot => AVirtualMachineStopReason::AVIRTUAL_MACHINE_REBOOT,
        DeathReason::Crash => AVirtualMachineStopReason::AVIRTUAL_MACHINE_CRASH,
        DeathReason::PvmFirmwarePublicKeyMismatch => {
            AVirtualMachineStopReason::AVIRTUAL_MACHINE_PVM_FIRMWARE_PUBLIC_KEY_MISMATCH
        }
        DeathReason::PvmFirmwareInstanceImageChanged => {
            AVirtualMachineStopReason::AVIRTUAL_MACHINE_PVM_FIRMWARE_INSTANCE_IMAGE_CHANGED
        }
        DeathReason::Hangup => AVirtualMachineStopReason::AVIRTUAL_MACHINE_HANGUP,
        _ => AVirtualMachineStopReason::AVIRTUAL_MACHINE_UNRECOGNISED,
    }
}

/// Wait until a virtual machine stops or the timeout elapses.
///
/// # Safety
/// `vm` must be a pointer returned by `AVirtualMachine_createRaw`. `timeout` must be a valid
/// pointer to a `struct timespec` object or null. `reason` must be a valid, non-null pointer to an
/// AVirtualMachineStopReason object.
#[no_mangle]
pub unsafe extern "C" fn AVirtualMachine_waitForStop(
    vm: *const VmInstance,
    timeout: *const timespec,
    reason: *mut AVirtualMachineStopReason,
) -> bool {
    // SAFETY: `vm` is assumed to be a valid, non-null pointer returned by
    // AVirtualMachine_create. It's the only reference to the object.
    let vm = unsafe { &*vm };

    let death_reason = if timeout.is_null() {
        vm.wait_for_death()
    } else {
        // SAFETY: `timeout` is assumed to be a valid pointer to a `struct timespec` object if
        // non-null.
        let timeout = unsafe { &*timeout };
        let timeout = Duration::new(timeout.tv_sec as u64, timeout.tv_nsec as u32);
        match vm.wait_for_death_with_timeout(timeout) {
            Some(death_reason) => death_reason,
            None => return false,
        }
    };

    // SAFETY: `reason` is assumed to be a valid, non-null pointer to an
    // AVirtualMachineStopReason object.
    unsafe { *reason = death_reason_to_stop_reason(death_reason) };
    true
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
