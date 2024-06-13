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
package android.system.virtualizationservice_internal;

import android.system.virtualizationcommon.Certificate;
import android.system.virtualizationservice.AssignableDevice;
import android.system.virtualizationservice.VirtualMachineDebugInfo;
import android.system.virtualizationservice_internal.AtomVmBooted;
import android.system.virtualizationservice_internal.AtomVmCreationRequested;
import android.system.virtualizationservice_internal.AtomVmExited;
import android.system.virtualizationservice_internal.IBoundDevice;
import android.system.virtualizationservice_internal.IGlobalVmContext;

interface IVirtualizationServiceInternal {
    /**
     * Removes the memlock rlimit of the calling process.
     *
     * The SELinux policy only allows this to succeed for virtmgr callers.
     */
    void removeMemlockRlimit();

    /**
     * Allocates global context for a new VM.
     *
     * This allocates VM's globally unique resources such as the CID.
     * The resources will not be recycled as long as there is a strong reference
     * to the returned object.
     */
    IGlobalVmContext allocateGlobalVmContext(int requesterDebugPid);

    /** Forwards a VmBooted atom to statsd. */
    void atomVmBooted(in AtomVmBooted atom);

    /** Forwards a VmCreationRequested atom to statsd. */
    void atomVmCreationRequested(in AtomVmCreationRequested atom);

    /** Forwards a VmExited atom to statsd. */
    void atomVmExited(in AtomVmExited atom);

    /** Get a list of all currently running VMs. */
    VirtualMachineDebugInfo[] debugListVms();

    /**
     * Requests a certificate chain for the provided certificate signing request (CSR).
     *
     * @param csr The certificate signing request.
     * @param requesterUid The UID of the app that requests remote attestation. The client VM to be
     *                     attested is owned by this app.
     *                     The uniqueness of the UID ensures that no two VMs owned by different apps
     *                     are able to correlate keys.
     * @param testMode Whether the request is for testing purposes.
     * @return A sequence of DER-encoded X.509 certificates that make up the attestation
     *         key's certificate chain. The attestation key is provided in the CSR.
     */
    Certificate[] requestAttestation(in byte[] csr, int requesterUid, in boolean testMode);

    /**
     * Provisions a key pair for the VM attestation testing, a fake certificate will be
     * associated to the fake key pair when the VM requests attestation in testing mode.
     *
     * The provisioned key pair will be used in the subsequent call to {@link #requestAttestation}
     * with testMode set to true.
     */
    void enableTestAttestation();

    /**
     * Returns {@code true} if the pVM remote attestation feature is supported
     */
    boolean isRemoteAttestationSupported();

    /**
     * Get a list of assignable devices.
     */
    AssignableDevice[] getAssignableDevices();

    /**
     * Bind given devices to vfio driver.
     *
     * @param devices paths of sysfs nodes of devices to assign.
     * @return a list of IBoundDevices representing VFIO bound devices.
     */
    IBoundDevice[] bindDevicesToVfioDriver(in String[] devices);

    /** Returns a read-only file descriptor of the VM DTBO file. */
    ParcelFileDescriptor getDtboFile();

    /**
     * Allocate an instance_id to the (newly created) VM.
     */
    byte[64] allocateInstanceId();

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

    // TODO(b/330257000): Remove these functions when a display service is running with binder RPC.
    void setDisplayService(IBinder ibinder);
    void clearDisplayService();
    IBinder waitDisplayService();

    /**
     * Create TAP network interface for a VM.
     * @param suffix of network interface name.
     * @return file descriptor of the TAP network interface.
     */
    ParcelFileDescriptor createTapInterface(String ifaceNameSuffix);

    /**
     * Delete TAP network interface created for a VM.
     * @param file descriptor of the TAP network interface.
     */
    void deleteTapInterface(in ParcelFileDescriptor tapFd);
}
