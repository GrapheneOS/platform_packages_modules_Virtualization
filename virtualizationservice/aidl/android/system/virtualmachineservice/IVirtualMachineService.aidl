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
package android.system.virtualmachineservice;

import android.hardware.security.secretkeeper.ISecretkeeper;
import android.system.virtualizationcommon.Certificate;
import android.system.virtualizationcommon.ErrorCode;

/** {@hide} */
interface IVirtualMachineService {
    /**
     * Port number that VirtualMachineService listens on connections from the guest VMs for the
     * tombtones
     */
    const int VM_TOMBSTONES_SERVICE_PORT = 2000;

    /**
     * Notifies that the payload has started.
     */
    void notifyPayloadStarted();

    /**
     * Notifies that the payload is ready to serve.
     */
    void notifyPayloadReady();

    /**
     * Notifies that the payload has finished.
     */
    void notifyPayloadFinished(int exitCode);

    /**
     * Notifies that an error has occurred inside the VM.
     */
    void notifyError(ErrorCode errorCode, in String message);

    /**
     * Requests a certificate chain for the provided certificate signing request (CSR).
     *
     * @param csr The certificate signing request.
     * @param testMode Whether the request is for test purposes.
     * @return A sequence of DER-encoded X.509 certificates that make up the attestation
     *         key's certificate chain. The attestation key is provided in the CSR.
     */
    Certificate[] requestAttestation(in byte[] csr, in boolean testMode);

    /**
     * Request connection to Secretkeeper. This is used by pVM to store rollback protected secrets.
     * Note that this returns error if Secretkeeper is not supported on device. Guest should check
     * that Secretkeeper is supported from Linux device tree before calling this.
     */
    ISecretkeeper getSecretkeeper();
}
