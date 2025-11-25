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

use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::Atom::Atom;
use android_system_virtualizationcommon::aidl::android::system::virtualizationcommon::DeathReason::DeathReason;
use anyhow::Result;
use log::{trace, warn};
use rustutils::android::system_properties::PropertyWatcher;
use statslog_virtualization_rust::{
    cgroup_memory_breach_reported, fsck_failed_reported, get_or_create_sk_secret_failed_reported,
    psi_monitor_failed_reported, stale_encryptedstore_detected, vm_booted, vm_creation_requested,
    vm_exited,
};

pub fn write_atom(atom: &Atom, vm_requester_uid: i32, vm_identifier: &str) {
    wait_for_statsd().unwrap_or_else(|e| warn!("failed to wait for statsd with error: {e}"));

    let result = match atom {
        Atom::CgroupMemoryBreachReported(atom) => {
            cgroup_memory_breach_reported::CgroupMemoryBreachReported {
                high_breach_count: atom.highBreachCount,
                high_memory_peak_mb: atom.highMemoryPeakMb,
                vm_requester_uid,
                vm_identifier,
            }
            .stats_write()
        }
        Atom::FsckFailedReported(atom) => fsck_failed_reported::FsckFailedReported {
            exit_code: atom.exitCode,
            vm_requester_uid,
            vm_identifier,
        }
        .stats_write(),
        Atom::GetOrCreateSkSecretFailedReported(atom) => {
            get_or_create_sk_secret_failed_reported::GetOrCreateSkSecretFailedReported {
                retry_count: atom.retryCount,
                vm_requester_uid,
                vm_identifier,
            }
            .stats_write()
        }
        Atom::PsiMonitorFailedReported(atom) => {
            psi_monitor_failed_reported::PsiMonitorFailedReported {
                exponential_backoff_seconds: atom.exponentialBackoffSeconds,
                vm_requester_uid,
                vm_identifier,
            }
            .stats_write()
        }
        Atom::StaleEncryptedstoreDetected(_) => {
            stale_encryptedstore_detected::StaleEncryptedstoreDetected {
                vm_requester_uid,
                vm_identifier,
            }
            .stats_write()
        }
        Atom::VmBooted(atom) => vm_booted::VmBooted {
            uid: vm_requester_uid,
            vm_identifier,
            elapsed_time_millis: atom.elapsedTimeMillis,
        }
        .stats_write(),
        Atom::VmCreationRequested(atom) => {
            let config_type = match atom.configType {
                x if x == vm_creation_requested::ConfigType::VirtualMachineAppConfig as i32 => {
                    vm_creation_requested::ConfigType::VirtualMachineAppConfig
                }
                x if x == vm_creation_requested::ConfigType::VirtualMachineRawConfig as i32 => {
                    vm_creation_requested::ConfigType::VirtualMachineRawConfig
                }
                _ => vm_creation_requested::ConfigType::UnknownConfig,
            };
            vm_creation_requested::VmCreationRequested {
                uid: vm_requester_uid,
                vm_identifier,
                hypervisor: vm_creation_requested::Hypervisor::Pkvm,
                is_protected: atom.isProtected,
                creation_succeeded: atom.creationSucceeded,
                binder_exception_code: atom.binderExceptionCode,
                config_type,
                num_cpus: atom.numCpus,
                cpu_affinity: "", // deprecated
                memory_mib: atom.memoryMib,
                apexes: &atom.apexes,
                // TODO(seungjaeyoo) Fill information about disk_image for raw config
            }
            .stats_write()
        }
        Atom::VmExited(atom) => {
            let death_reason = match atom.deathReason {
                DeathReason::INFRASTRUCTURE_ERROR => vm_exited::DeathReason::InfrastructureError,
                DeathReason::KILLED => vm_exited::DeathReason::Killed,
                DeathReason::UNKNOWN => vm_exited::DeathReason::Unknown,
                DeathReason::SHUTDOWN => vm_exited::DeathReason::Shutdown,
                DeathReason::START_FAILED => vm_exited::DeathReason::Error,
                DeathReason::REBOOT => vm_exited::DeathReason::Reboot,
                DeathReason::CRASH => vm_exited::DeathReason::Crash,
                DeathReason::PVM_FIRMWARE_PUBLIC_KEY_MISMATCH => {
                    vm_exited::DeathReason::PvmFirmwarePublicKeyMismatch
                }
                DeathReason::PVM_FIRMWARE_INSTANCE_IMAGE_CHANGED => {
                    vm_exited::DeathReason::PvmFirmwareInstanceImageChanged
                }
                DeathReason::MICRODROID_FAILED_TO_CONNECT_TO_VIRTUALIZATION_SERVICE => {
                    vm_exited::DeathReason::MicrodroidFailedToConnectToVirtualizationService
                }
                DeathReason::MICRODROID_PAYLOAD_HAS_CHANGED => {
                    vm_exited::DeathReason::MicrodroidPayloadHasChanged
                }
                DeathReason::MICRODROID_PAYLOAD_VERIFICATION_FAILED => {
                    vm_exited::DeathReason::MicrodroidPayloadVerificationFailed
                }
                DeathReason::MICRODROID_INVALID_PAYLOAD_CONFIG => {
                    vm_exited::DeathReason::MicrodroidInvalidPayloadConfig
                }
                DeathReason::MICRODROID_UNKNOWN_RUNTIME_ERROR => {
                    vm_exited::DeathReason::MicrodroidUnknownRuntimeError
                }
                DeathReason::HANGUP => vm_exited::DeathReason::Hangup,
                _ => vm_exited::DeathReason::Unknown,
            };

            vm_exited::VmExited {
                uid: vm_requester_uid,
                vm_identifier,
                elapsed_time_millis: atom.elapsedTimeMillis,
                death_reason,
                guest_time_millis: atom.guestTimeMillis,
                rss_vm_kb: atom.rssKb,
                rss_crosvm_kb: atom.rssKb,
                exit_signal: atom.exitSignal,
            }
            .stats_write()
        }
    };
    match result {
        Err(e) => warn!("statslog_rust failed with error: {e}"),
        Ok(_) => trace!("statslog_rust succeeded for virtualization service"),
    }
}

fn wait_for_statsd() -> Result<()> {
    PropertyWatcher::new("init.svc.statsd")?.wait_for_value("running", None)?;
    Ok(())
}
