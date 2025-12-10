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

//! Functions for creating and collecting atoms.

use crate::aidl;
use crate::crosvm::VmMetric;
use crate::get_calling_uid;
use crate::virtualmachine;
use anyhow::{anyhow, Result};
use binder::{ParcelFileDescriptor, Status, Strong};
use log::{info, warn};
use microdroid_payload_config::VmPayloadConfig;
use statslog_virtualization_rust::vm_creation_requested;
use std::time::{Duration, SystemTime};
use zip::ZipArchive;

const INVALID_NUM_CPUS: i32 = -1;

fn get_apex_list(config: &aidl::VirtualMachineAppConfig) -> String {
    match &config.payload {
        aidl::Payload::PayloadConfig(_) => String::new(),
        aidl::Payload::ConfigPath(config_path) => {
            let vm_payload_config = get_vm_payload_config(config.apk.as_ref(), config_path);
            if let Ok(vm_payload_config) = vm_payload_config {
                vm_payload_config
                    .apexes
                    .iter()
                    .map(|x| x.name.clone())
                    .collect::<Vec<String>>()
                    .join(":")
            } else {
                "INFO: Can't get VmPayloadConfig".to_owned()
            }
        }
    }
}

fn get_vm_payload_config(
    apk_fd: Option<&ParcelFileDescriptor>,
    config_path: &str,
) -> Result<VmPayloadConfig> {
    let apk = apk_fd.as_ref().ok_or_else(|| anyhow!("APK is none"))?;
    let apk_file = virtualmachine::clone_file(apk)?;
    let mut apk_zip = ZipArchive::new(&apk_file)?;
    let config_file = apk_zip.by_name(config_path)?;
    let vm_payload_config: VmPayloadConfig = serde_json::from_reader(config_file)?;
    Ok(vm_payload_config)
}

fn get_duration(vm_start_timestamp: Option<SystemTime>) -> Duration {
    match vm_start_timestamp {
        Some(vm_start_timestamp) => vm_start_timestamp.elapsed().unwrap_or_default(),
        None => Duration::default(),
    }
}

// Returns the number of CPUs configured in the host system.
// This matches how crosvm determines the number of logical cores.
// For telemetry purposes only.
pub(crate) fn get_num_cpus() -> Option<usize> {
    // SAFETY: Only integer constants passed back and forth.
    let ret = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
    if ret > 0 {
        ret.try_into().ok()
    } else {
        None
    }
}

fn get_num_vcpus(cpu_options: &aidl::CpuOptions) -> i32 {
    match cpu_options.cpuTopology {
        aidl::CpuTopology::MatchHost(_) => {
            get_num_cpus().and_then(|v| v.try_into().ok()).unwrap_or_else(|| {
                warn!("Failed to determine the number of CPUs in the host");
                INVALID_NUM_CPUS
            })
        }
        aidl::CpuTopology::CpuCount(count) => count,
    }
}

/// Write the stats of VMCreation to statsd
pub fn write_vm_creation_stats(
    config: &aidl::VirtualMachineConfig,
    is_protected: bool,
    ret: &binder::Result<Strong<dyn aidl::IVirtualMachine>>,
) {
    if cfg!(early) {
        info!("Writing VmCreationRequested atom for early VMs is not implemented; skipping");
        return;
    }
    let creation_succeeded;
    let binder_exception_code;
    match ret {
        Ok(_) => {
            creation_succeeded = true;
            binder_exception_code = Status::ok().exception_code() as i32;
        }
        Err(ref e) => {
            creation_succeeded = false;
            binder_exception_code = e.exception_code() as i32;
        }
    }

    let (vm_identifier, config_type, num_cpus, memory_mib, apexes) = match config {
        aidl::VirtualMachineConfig::AppConfig(config) => (
            config.name.clone(),
            vm_creation_requested::ConfigType::VirtualMachineAppConfig,
            get_num_vcpus(&config.cpuOptions),
            config.memoryMib,
            get_apex_list(config),
        ),
        aidl::VirtualMachineConfig::RawConfig(config) => (
            config.name.clone(),
            vm_creation_requested::ConfigType::VirtualMachineRawConfig,
            get_num_vcpus(&config.cpuOptions),
            config.memoryMib,
            String::new(),
        ),
    };

    let uid = get_calling_uid() as i32;

    let atom = aidl::Atom::VmCreationRequested(aidl::VmCreationRequested {
        isProtected: is_protected,
        creationSucceeded: creation_succeeded,
        binderExceptionCode: binder_exception_code,
        configType: config_type as i32,
        numCpus: num_cpus,
        memoryMib: memory_mib,
        apexes,
    });

    info!("Writing VmCreationRequested atom into statsd.");
    virtualmachine::global_service()
        .unwrap()
        .forwardAtom(&atom, uid, &vm_identifier)
        .unwrap_or_else(|e| {
            warn!("Failed to write VmCreationRequested atom: {e}");
        });
}

/// Write the stats of VM boot to statsd
pub fn write_vm_booted_stats(
    uid: i32,
    vm_identifier: &str,
    vm_start_timestamp: Option<SystemTime>,
) {
    if cfg!(early) {
        info!("Writing VmCreationRequested atom for early VMs is not implemented; skipping");
        return;
    }

    let vm_identifier = vm_identifier.to_owned();
    let duration = get_duration(vm_start_timestamp);

    let atom =
        aidl::Atom::VmBooted(aidl::VmBooted { elapsedTimeMillis: duration.as_millis() as i64 });

    info!("Writing VmBooted atom into statsd.");
    virtualmachine::global_service()
        .unwrap()
        .forwardAtom(&atom, uid, &vm_identifier)
        .unwrap_or_else(|e| {
            warn!("Failed to write VmBooted atom: {e}");
        });
}

/// Write the stats of VM exit to statsd
pub fn write_vm_exited_stats_sync(
    uid: i32,
    vm_identifier: &str,
    reason: aidl::DeathReason,
    exit_signal: Option<i32>,
    vm_metric: &VmMetric,
) {
    if cfg!(early) {
        info!("Writing VmExited atom for early VMs is not implemented; skipping");
        return;
    }
    let elapsed_time_millis = get_duration(vm_metric.start_timestamp).as_millis() as i64;
    let guest_time_millis = vm_metric.cpu_guest_time.unwrap_or_default();
    let rss_kb = vm_metric.rss.unwrap_or_default();

    let atom = aidl::Atom::VmExited(aidl::VmExited {
        elapsedTimeMillis: elapsed_time_millis,
        deathReason: reason,
        guestTimeMillis: guest_time_millis,
        rssKb: rss_kb,
        exitSignal: exit_signal.unwrap_or_default(),
    });

    info!("Writing VmExited atom into statsd.");
    virtualmachine::global_service()
        .unwrap()
        .forwardAtom(&atom, uid, vm_identifier)
        .unwrap_or_else(|e| {
            warn!("Failed to write VmExited atom: {e}");
        });
}
