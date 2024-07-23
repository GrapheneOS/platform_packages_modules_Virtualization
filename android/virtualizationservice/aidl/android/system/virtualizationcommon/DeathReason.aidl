/*
 * Copyright 2022 The Android Open Source Project
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
package android.system.virtualizationcommon;

/**
 * The reason why a VM died.
 */
@Backing(type="int")
enum DeathReason {
    /** There was an error waiting for the VM. */
    INFRASTRUCTURE_ERROR = 0,
    /** The VM was killed. */
    KILLED = 1,
    /** The VM died for an unknown reason. */
    UNKNOWN = 2,
    /** The VM requested to shut down. */
    SHUTDOWN = 3,
    /** crosvm had an error starting the VM. */
    START_FAILED = 4,
    /** The VM requested to reboot, possibly as the result of a kernel panic. */
    REBOOT = 5,
    /** The VM or crosvm crashed. */
    CRASH = 6,
    /** The pVM firmware failed to verify the VM because the public key doesn't match. */
    PVM_FIRMWARE_PUBLIC_KEY_MISMATCH = 7,
    /** The pVM firmware failed to verify the VM because the instance image changed. */
    PVM_FIRMWARE_INSTANCE_IMAGE_CHANGED = 8,
    // 9 & 10 intentionally removed.
    /** The microdroid failed to connect to VirtualizationService's RPC server. */
    MICRODROID_FAILED_TO_CONNECT_TO_VIRTUALIZATION_SERVICE = 11,
    /** The payload for microdroid is changed. */
    MICRODROID_PAYLOAD_HAS_CHANGED = 12,
    /** The microdroid failed to verify given payload APK. */
    MICRODROID_PAYLOAD_VERIFICATION_FAILED = 13,
    /** The VM config for microdroid is invalid (e.g. missing tasks). */
    MICRODROID_INVALID_PAYLOAD_CONFIG = 14,
    /** There was a runtime error while running microdroid manager. */
    MICRODROID_UNKNOWN_RUNTIME_ERROR = 15,
    /** The VM killed due to hangup */
    HANGUP = 16,
    /** The VCPU stalled */
    WATCHDOG_REBOOT = 17,
}
