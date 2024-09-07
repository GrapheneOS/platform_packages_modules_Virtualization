// Copyright 2023, The Android Open Source Project
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

//! Implementation of the AIDL interface of VfioHandler.

use anyhow::{anyhow, Context};
use android_system_virtualizationservice_internal::aidl::android::system::virtualizationservice_internal::IBoundDevice::{IBoundDevice, BnBoundDevice};
use android_system_virtualizationservice_internal::aidl::android::system::virtualizationservice_internal::IVfioHandler::IVfioHandler;
use android_system_virtualizationservice_internal::aidl::android::system::virtualizationservice_internal::IVfioHandler::VfioDev::VfioDev;
use android_system_virtualizationservice_internal::binder::ParcelFileDescriptor;
use binder::{self, BinderFeatures, ExceptionCode, Interface, IntoBinderResult, Strong};
use log::error;
use std::fs::{read_link, write, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::sync::LazyLock;
use std::path::{Path, PathBuf};
use rustutils::system_properties;
use zerocopy::{
    byteorder::{BigEndian, U32},
    FromZeroes,
    FromBytes,
};

// Device bound to VFIO driver.
struct BoundDevice {
    sysfs_path: String,
    dtbo_label: String,
}

impl Interface for BoundDevice {}

impl IBoundDevice for BoundDevice {
    fn getSysfsPath(&self) -> binder::Result<String> {
        Ok(self.sysfs_path.clone())
    }

    fn getDtboLabel(&self) -> binder::Result<String> {
        Ok(self.dtbo_label.clone())
    }
}

impl Drop for BoundDevice {
    fn drop(&mut self) {
        unbind_device(Path::new(&self.sysfs_path)).unwrap_or_else(|e| {
            error!("did not restore {} driver: {}", self.sysfs_path, e);
        });
    }
}

impl BoundDevice {
    fn new_binder(sysfs_path: String, dtbo_label: String) -> Strong<dyn IBoundDevice> {
        BnBoundDevice::new_binder(BoundDevice { sysfs_path, dtbo_label }, BinderFeatures::default())
    }
}

#[derive(Debug, Default)]
pub struct VfioHandler {}

impl VfioHandler {
    pub fn init() -> VfioHandler {
        VfioHandler::default()
    }
}

impl Interface for VfioHandler {}

impl IVfioHandler for VfioHandler {
    fn bindDevicesToVfioDriver(
        &self,
        devices: &[VfioDev],
    ) -> binder::Result<Vec<Strong<dyn IBoundDevice>>> {
        // permission check is already done by IVirtualizationServiceInternal.
        if !*IS_VFIO_SUPPORTED {
            return Err(anyhow!("VFIO-platform not supported"))
                .or_binder_exception(ExceptionCode::UNSUPPORTED_OPERATION);
        }
        devices
            .iter()
            .map(|d| {
                bind_device(Path::new(&d.sysfsPath))?;
                Ok(BoundDevice::new_binder(d.sysfsPath.clone(), d.dtboLabel.clone()))
            })
            .collect::<binder::Result<Vec<_>>>()
    }

    fn writeVmDtbo(&self, dtbo_fd: &ParcelFileDescriptor) -> binder::Result<()> {
        let dtbo_path = get_dtbo_img_path()?;
        let mut dtbo_img = File::open(dtbo_path)
            .context("Failed to open DTBO partition")
            .or_service_specific_exception(-1)?;

        let dt_table_header = get_dt_table_header(&mut dtbo_img)?;
        let vm_dtbo_idx = system_properties::read("ro.boot.hypervisor.vm_dtbo_idx")
            .context("Failed to read vm_dtbo_idx")
            .or_service_specific_exception(-1)?
            .ok_or_else(|| anyhow!("vm_dtbo_idx is none"))
            .or_service_specific_exception(-1)?;
        let vm_dtbo_idx = vm_dtbo_idx
            .parse()
            .context("vm_dtbo_idx is not an integer")
            .or_service_specific_exception(-1)?;
        let dt_table_entry = get_dt_table_entry(&mut dtbo_img, &dt_table_header, vm_dtbo_idx)?;
        write_vm_full_dtbo_from_img(&mut dtbo_img, &dt_table_entry, dtbo_fd)?;
        Ok(())
    }
}

const DEV_VFIO_PATH: &str = "/dev/vfio/vfio";
const SYSFS_PLATFORM_DEVICES_PATH: &str = "/sys/devices/platform/";
const VFIO_PLATFORM_DRIVER_PATH: &str = "/sys/bus/platform/drivers/vfio-platform";
const SYSFS_PLATFORM_DRIVERS_PROBE_PATH: &str = "/sys/bus/platform/drivers_probe";
const DT_TABLE_MAGIC: u32 = 0xd7b7ab1e;
const VFIO_PLATFORM_DRIVER_NAME: &str = "vfio-platform";
// To remove the override and match the device driver by "compatible" string again,
// driver_override file must be cleared. Writing an empty string (same as
// `echo -n "" > driver_override`) won't' clear the file, so append a newline char.
const DEFAULT_DRIVER: &str = "\n";

/// The structure of DT table header in dtbo.img.
/// https://source.android.com/docs/core/architecture/dto/partitions
#[repr(C)]
#[derive(Debug, FromZeroes, FromBytes)]
struct DtTableHeader {
    /// DT_TABLE_MAGIC
    magic: U32<BigEndian>,
    /// includes dt_table_header + all dt_table_entry and all dtb/dtbo
    _total_size: U32<BigEndian>,
    /// sizeof(dt_table_header)
    header_size: U32<BigEndian>,
    /// sizeof(dt_table_entry)
    dt_entry_size: U32<BigEndian>,
    /// number of dt_table_entry
    dt_entry_count: U32<BigEndian>,
    /// offset to the first dt_table_entry from head of dt_table_header
    dt_entries_offset: U32<BigEndian>,
    /// flash page size we assume
    _page_size: U32<BigEndian>,
    /// DTBO image version, the current version is 0. The version will be
    /// incremented when the dt_table_header struct is updated.
    _version: U32<BigEndian>,
}

/// The structure of each DT table entry (v0) in dtbo.img.
/// https://source.android.com/docs/core/architecture/dto/partitions
#[repr(C)]
#[derive(Debug, FromZeroes, FromBytes)]
struct DtTableEntry {
    /// size of each DT
    dt_size: U32<BigEndian>,
    /// offset from head of dt_table_header
    dt_offset: U32<BigEndian>,
    /// optional, must be zero if unused
    _id: U32<BigEndian>,
    /// optional, must be zero if unused
    _rev: U32<BigEndian>,
    /// optional, must be zero if unused
    _custom: [U32<BigEndian>; 4],
}

static IS_VFIO_SUPPORTED: LazyLock<bool> = LazyLock::new(|| {
    Path::new(DEV_VFIO_PATH).exists() && Path::new(VFIO_PLATFORM_DRIVER_PATH).exists()
});

fn check_platform_device(path: &Path) -> binder::Result<()> {
    if !path.exists() {
        return Err(anyhow!("no such device {path:?}"))
            .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
    }

    if !path.starts_with(SYSFS_PLATFORM_DEVICES_PATH) {
        return Err(anyhow!("{path:?} is not a platform device"))
            .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
    }

    Ok(())
}

fn get_device_iommu_group(path: &Path) -> Option<u64> {
    let group_path = read_link(path.join("iommu_group")).ok()?;
    let group = group_path.file_name()?;
    group.to_str()?.parse().ok()
}

fn current_driver(path: &Path) -> Option<String> {
    let driver_path = read_link(path.join("driver")).ok()?;
    let bound_driver = driver_path.file_name()?;
    bound_driver.to_str().map(str::to_string)
}

// Try to bind device driver by writing its name to driver_override and triggering driver probe.
fn try_bind_driver(path: &Path, driver: &str) -> binder::Result<()> {
    if Some(driver) == current_driver(path).as_deref() {
        // already bound
        return Ok(());
    }

    // unbind
    let Some(device) = path.file_name() else {
        return Err(anyhow!("can't get device name from {path:?}"))
            .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
    };
    let Some(device_str) = device.to_str() else {
        return Err(anyhow!("invalid filename {device:?}"))
            .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT);
    };
    let unbind_path = path.join("driver/unbind");
    if unbind_path.exists() {
        write(&unbind_path, device_str.as_bytes())
            .with_context(|| format!("could not unbind {device_str}"))
            .or_service_specific_exception(-1)?;
    }
    if path.join("driver").exists() {
        return Err(anyhow!("could not unbind {device_str}")).or_service_specific_exception(-1);
    }

    // bind to new driver
    write(path.join("driver_override"), driver.as_bytes())
        .with_context(|| format!("could not bind {device_str} to '{driver}' driver"))
        .or_service_specific_exception(-1)?;

    write(SYSFS_PLATFORM_DRIVERS_PROBE_PATH, device_str.as_bytes())
        .with_context(|| format!("could not write {device_str} to drivers-probe"))
        .or_service_specific_exception(-1)?;

    // final check
    let new_driver = current_driver(path);
    if new_driver.is_none() || Some(driver) != new_driver.as_deref() && driver != DEFAULT_DRIVER {
        return Err(anyhow!("{path:?} still not bound to '{driver}' driver"))
            .or_service_specific_exception(-1);
    }

    Ok(())
}

fn bind_device(path: &Path) -> binder::Result<()> {
    let path = path
        .canonicalize()
        .with_context(|| format!("can't canonicalize {path:?}"))
        .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT)?;

    check_platform_device(&path)?;
    try_bind_driver(&path, VFIO_PLATFORM_DRIVER_NAME)?;

    if get_device_iommu_group(&path).is_none() {
        Err(anyhow!("can't get iommu group for {path:?}")).or_service_specific_exception(-1)
    } else {
        Ok(())
    }
}

fn unbind_device(path: &Path) -> binder::Result<()> {
    let path = path
        .canonicalize()
        .with_context(|| format!("can't canonicalize {path:?}"))
        .or_binder_exception(ExceptionCode::ILLEGAL_ARGUMENT)?;

    check_platform_device(&path)?;
    try_bind_driver(&path, DEFAULT_DRIVER)?;

    if Some(VFIO_PLATFORM_DRIVER_NAME) == current_driver(&path).as_deref() {
        Err(anyhow!("{path:?} still bound to vfio driver")).or_service_specific_exception(-1)
    } else {
        Ok(())
    }
}

fn get_dtbo_img_path() -> binder::Result<PathBuf> {
    let slot_suffix = system_properties::read("ro.boot.slot_suffix")
        .context("Failed to read ro.boot.slot_suffix")
        .or_service_specific_exception(-1)?
        .ok_or_else(|| anyhow!("slot_suffix is none"))
        .or_service_specific_exception(-1)?;
    Ok(PathBuf::from(format!("/dev/block/by-name/dtbo{slot_suffix}")))
}

fn read_values(file: &mut File, size: usize, offset: u64) -> binder::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))
        .context("Cannot seek the offset")
        .or_service_specific_exception(-1)?;
    let mut buffer = vec![0_u8; size];
    file.read_exact(&mut buffer)
        .context("Failed to read buffer")
        .or_service_specific_exception(-1)?;
    Ok(buffer)
}

fn get_dt_table_header(file: &mut File) -> binder::Result<DtTableHeader> {
    let values = read_values(file, size_of::<DtTableHeader>(), 0)?;
    let dt_table_header = DtTableHeader::read_from(values.as_slice())
        .context("DtTableHeader is invalid")
        .or_service_specific_exception(-1)?;
    if dt_table_header.magic.get() != DT_TABLE_MAGIC
        || dt_table_header.header_size.get() as usize != size_of::<DtTableHeader>()
    {
        return Err(anyhow!("DtTableHeader is invalid")).or_service_specific_exception(-1);
    }
    Ok(dt_table_header)
}

fn get_dt_table_entry(
    file: &mut File,
    header: &DtTableHeader,
    index: u32,
) -> binder::Result<DtTableEntry> {
    if index >= header.dt_entry_count.get() {
        return Err(anyhow!("Invalid dtbo index {index}")).or_service_specific_exception(-1);
    }
    let Some(prev_dt_entry_total_size) = header.dt_entry_size.get().checked_mul(index) else {
        return Err(anyhow!("Unexpected arithmetic result"))
            .or_binder_exception(ExceptionCode::ILLEGAL_STATE);
    };
    let Some(dt_entry_offset) =
        prev_dt_entry_total_size.checked_add(header.dt_entries_offset.get())
    else {
        return Err(anyhow!("Unexpected arithmetic result"))
            .or_binder_exception(ExceptionCode::ILLEGAL_STATE);
    };
    let values = read_values(file, size_of::<DtTableEntry>(), dt_entry_offset.into())?;
    let dt_table_entry = DtTableEntry::read_from(values.as_slice())
        .with_context(|| format!("DtTableEntry at index {index} is invalid."))
        .or_service_specific_exception(-1)?;
    Ok(dt_table_entry)
}

fn write_vm_full_dtbo_from_img(
    dtbo_img_file: &mut File,
    entry: &DtTableEntry,
    dtbo_fd: &ParcelFileDescriptor,
) -> binder::Result<()> {
    let dt_size = entry
        .dt_size
        .get()
        .try_into()
        .context("Failed to convert type")
        .or_binder_exception(ExceptionCode::ILLEGAL_STATE)?;
    let buffer = read_values(dtbo_img_file, dt_size, entry.dt_offset.get().into())?;

    let mut dtbo_fd = File::from(
        dtbo_fd
            .as_ref()
            .try_clone()
            .context("Failed to create File from ParcelFileDescriptor")
            .or_binder_exception(ExceptionCode::BAD_PARCELABLE)?,
    );

    dtbo_fd
        .write_all(&buffer)
        .context("Failed to write dtbo file")
        .or_service_specific_exception(-1)?;
    Ok(())
}
