/*
 * Copyright 2024 The Android Open Source Project
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

package com.android.virt.vm_attestation.testservice;

/** {@hide} */
interface IAttestationService {
    const int PORT = 5679;

    /**
     * The status of the attestation.
     *
     * The status here maps to the status defined in
     * vm_payload/include/vm_payload.h
     */
    @Backing(type="int")
    enum AttestationStatus {
        /** The remote attestation completes successfully. */
        OK = 0,

        /** The challenge size is not between 0 and 64. */
        ERROR_INVALID_CHALLENGE = 1,

        /** Failed to attest the VM. Please retry at a later time. */
        ERROR_ATTESTATION_FAILED = 2,

        /** Remote attestation is not supported in the current environment. */
        ERROR_UNSUPPORTED = 3,
    }

    /**
     * The result of signing a message with the attested key.
     */
    parcelable SigningResult {
        /** The DER-encoded ECDSA signature of the message. */
        byte[] signature;

        /** The DER-encoded attestation X509 certificate chain. */
        byte[] certificateChain;

        /** The status of the attestation. */
        AttestationStatus status;
    }

    /**
     * Requests attestation with {@link AVmPayload_requestAttestation} API and signs the
     * given message with the attested key.
     *
     * The remotely provisioned keys are retrieved from RKPD and are provisioned from the
     * real RKP server.
     *
     * @param challenge the challenge to include in the attestation output.
     * @param message the message to sign.
     * @return the result of signing the message with the attested key.
     */
    SigningResult signWithAttestationKey(in byte[] challenge, in byte[] message);

    /**
     * Requests attestation for testing with {@link AVmPayload_requestAttestationForTesting} API.
     *
     * A fake key pair should be provisioned with the call to
     * {@link VirtualMachine#enableTestAttestation()} before calling this method.
     *
     * The attestation result will be cached in the VM and can be validated with
     * {@link #validateAttestationResult}.
     */
    void requestAttestationForTesting();

    /**
     * Validates the attestation result returned by the last call to
     * {@link #requestAttestationForTesting}.
     */
    void validateAttestationResult();
}
