/*
 * Copyright 2021 The Android Open Source Project
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
package android.system.virtualizationservice;

import android.system.virtualizationservice.AssignableDevice;
import android.system.virtualizationservice.IVirtualMachine;
import android.system.virtualizationservice.PartitionType;
import android.system.virtualizationservice.VirtualMachineConfig;
import android.system.virtualizationservice.VirtualMachineDebugInfo;

interface IVirtualizationService {
    const String FEATURE_DICE_CHANGES = "com.android.kvm.DICE_CHANGES";
    const String FEATURE_LLPVM_CHANGES = "com.android.kvm.LLPVM_CHANGES";
    const String FEATURE_MULTI_TENANT = "com.android.kvm.MULTI_TENANT";
    const String FEATURE_NETWORK = "com.android.kvm.NETWORK";
    const String FEATURE_REMOTE_ATTESTATION = "com.android.kvm.REMOTE_ATTESTATION";
    const String FEATURE_VENDOR_MODULES = "com.android.kvm.VENDOR_MODULES";

    /**
     * Create the VM with the given config file, and return a handle to it ready to start it. If
     * `consoleOutFd` is provided then console output from the VM will be sent to it. If
     * `consoleInFd` is provided then console input to the VM will be read from it. If `osLogFd` is
     * provided then the OS-level logs will be sent to it. `osLogFd` is supported only when the OS
     * running in the VM has the logging system. In case of Microdroid, the logging system is logd.
     */
    IVirtualMachine createVm(in VirtualMachineConfig config,
            in @nullable ParcelFileDescriptor consoleOutFd,
            in @nullable ParcelFileDescriptor consoleInFd,
            in @nullable ParcelFileDescriptor osLogFd);

    /**
     * Allocate an instance_id to the (newly created) VM.
     */
    byte[64] allocateInstanceId();

    /**
     * Initialise an empty partition image of the given size to be used as a writable partition.
     *
     * The file must be open with both read and write permissions, and should be a new empty file.
     */
    void initializeWritablePartition(
            in ParcelFileDescriptor imageFd, long sizeBytes, PartitionType type);

    /**
     * Create or update an idsig file that digests the given APK file. The idsig file follows the
     * idsig format that is defined by the APK Signature Scheme V4. The idsig file is not updated
     * when it is up to date with the input file, which is checked by comparing the
     * signing_info.apk_digest field in the idsig file with the signer.signed_data.digests.digest
     * field in the input APK file.
     */
    void createOrUpdateIdsigFile(in ParcelFileDescriptor inputFd, in ParcelFileDescriptor idsigFd);

    /**
     * Get a list of all currently running VMs. This method is only intended for debug purposes,
     * and as such is only permitted from the shell user.
     */
    VirtualMachineDebugInfo[] debugListVms();

    /**
     * Get a list of assignable device types.
     */
    AssignableDevice[] getAssignableDevices();

    /**
     * Get a list of supported OSes.
     */
    String[] getSupportedOSList();

    /** Returns whether given feature is enabled. */
    boolean isFeatureEnabled(in String feature);

    /**
     * Provisions a key pair for the VM attestation testing, a fake certificate will be
     * associated to the fake key pair when the VM requests attestation in testing mode.
     */
    void enableTestAttestation();

    /**
     * Returns {@code true} if the pVM remote attestation feature is supported
     */
    boolean isRemoteAttestationSupported();

    /**
     * Check if Updatable VM feature is supported by AVF. Updatable VM allows secrets and data of
     * a VM instance to be accessible even after updates of boot images and apks.
     * For more info see packages/modules/Virtualization/docs/updatable_vm.md
     */
    boolean isUpdatableVmSupported();

    /**
     * Notification that state associated with a VM should be removed.
     *
     * @param instanceId The ID for the VM.
     */
    void removeVmInstance(in byte[64] instanceId);

    /**
     * Notification that ownership of a VM has been claimed by the caller.  Note that no permission
     * checks (with respect to the previous owner) are performed.
     *
     * @param instanceId The ID for the VM.
     */
    void claimVmInstance(in byte[64] instanceId);
}
