/*
 * Copyright (C) 2022 The Android Open Source Project
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

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/cdefs.h>

#include "vm_main.h"

__BEGIN_DECLS

typedef struct AIBinder AIBinder;

/**
 * Introduced in API 35.
 * Remote attestation result if the attestation succeeds.
 */
typedef struct AVmAttestationResult AVmAttestationResult;

/**
 * Introduced in API 35.
 * Remote attestation status types returned from remote attestation functions.
 */
typedef enum AVmAttestationStatus : int32_t {
    /** The remote attestation completes successfully. */
    ATTESTATION_OK = 0,

    /** The challenge size is not between 0 and 64. */
    ATTESTATION_ERROR_INVALID_CHALLENGE = -10001,

    /** Failed to attest the VM. Please retry at a later time. */
    ATTESTATION_ERROR_ATTESTATION_FAILED = -10002,

    /** Remote attestation is not supported in the current environment. */
    ATTESTATION_ERROR_UNSUPPORTED = -10003,
} AVmAttestationStatus;

/**
 * Notifies the host that the payload is ready.
 *
 * If the host app has set a `VirtualMachineCallback` for the VM, its
 * `onPayloadReady` method will be called.
 *
 * Note that subsequent calls to this function after the first have no effect;
 * `onPayloadReady` is never called more than once.
 */
void AVmPayload_notifyPayloadReady(void);

/**
 * Runs a binder RPC server, serving the supplied binder service implementation on the given vsock
 * port.
 *
 * If and when the server is ready for connections (it is listening on the port), `on_ready` is
 * called to allow appropriate action to be taken - e.g. to notify clients that they may now
 * attempt to connect with `AVmPayload_notifyPayloadReady`.
 *
 * Note that this function does not return. The calling thread joins the binder
 * thread pool to handle incoming messages.
 *
 * \param service the service to bind to the given port.
 * \param port vsock port.
 * \param on_ready the callback to execute once the server is ready for connections. If not null the
 * callback will be called at most once.
 * \param param parameter to be passed to the `on_ready` callback.
 */
__attribute__((noreturn)) void AVmPayload_runVsockRpcServer(
        AIBinder* _Nonnull service, uint32_t port,
        void (*_Nullable on_ready)(void* _Nullable param), void* _Nullable param);

/**
 * Returns all or part of a 32-byte secret that is bound to this unique VM
 * instance and the supplied identifier. The secret can be used e.g. as an
 * encryption key.
 *
 * Every VM has a secret that is derived from a device-specific value known to
 * the hypervisor, the code that runs in the VM and its non-modifiable
 * configuration; it is not made available to the host OS.
 *
 * This function performs a further derivation from the VM secret and the
 * supplied identifier. As long as the VM identity doesn't change the same value
 * will be returned for the same identifier, even if the VM is stopped &
 * restarted or the device rebooted.
 *
 * If multiple secrets are required for different purposes, a different
 * identifier should be used for each. The identifiers otherwise are arbitrary
 * byte sequences and do not need to be kept secret; typically they are
 * hardcoded in the calling code.
 *
 * \param identifier identifier of the secret to return.
 * \param identifier_size size of the secret identifier.
 * \param secret pointer to size bytes where the secret is written.
 * \param size number of bytes of the secret to get, <= 32.
 */
void AVmPayload_getVmInstanceSecret(const void* _Nonnull identifier, size_t identifier_size,
                                    void* _Nonnull secret, size_t size);

/**
 * Gets the path to the APK contents. It is a directory, under which are
 * the unzipped contents of the APK containing the payload, all read-only
 * but accessible to the payload.
 *
 * \return the path to the APK contents. The returned string should not be
 * deleted or freed by the application. The string remains valid for the
 * lifetime of the VM.
 */
const char* _Nonnull AVmPayload_getApkContentsPath(void);

/**
 * Gets the path to the encrypted persistent storage for the VM, if any. This is
 * a directory under which any files or directories created will be stored on
 * behalf of the VM by the host app. All data is encrypted using a key known
 * only to the VM, so the host cannot decrypt it, but may delete it.
 *
 * \return the path to the encrypted storage directory, or NULL if no encrypted
 * storage was requested in the VM configuration. If non-null the returned
 * string should not be deleted or freed by the application and remains valid
 * for the lifetime of the VM.
 */
const char* _Nullable AVmPayload_getEncryptedStoragePath(void);

/**
 * Requests the remote attestation of the client VM.
 *
 * The challenge will be included in the certificate chain in the attestation result,
 * serving as proof of the freshness of the result.
 *
 * \param challenge A pointer to the challenge buffer.
 * \param challenge_size size of the challenge. The maximum supported challenge size is
 *          64 bytes. The status ATTESTATION_ERROR_INVALID_CHALLENGE will be returned if
 *          an invalid challenge is passed.
 * \param result The remote attestation result will be filled here if the attestation
 *               succeeds. The result remains valid until it is freed with
 *              `AVmPayload_freeAttestationResult`.
 *
 * \return ATTESTATION_OK upon successful attestation.
 */
AVmAttestationStatus AVmPayload_requestAttestation(const void* _Nonnull challenge,
                                                   size_t challenge_size,
                                                   AVmAttestationResult* _Nullable* _Nonnull result)
        __INTRODUCED_IN(__ANDROID_API_V__);

/**
 * Converts the return value from `AVmPayload_requestAttestation` to a text string
 * representing the status code.
 *
 * \return a constant string value representing the status code. The string should not
 * be deleted or freed by the application and remains valid for the lifetime of the VM.
 */
const char* _Nonnull AVmAttestationStatus_toString(AVmAttestationStatus status)
        __INTRODUCED_IN(__ANDROID_API_V__);

/**
 * Frees all the data owned by the provided attestation result, including the result itself.
 *
 * Callers should ensure to invoke this API only once on a valid attestation result
 * returned by `AVmPayload_requestAttestation` to avoid undefined behavior.
 *
 * \param result A pointer to the attestation result.
 */
void AVmAttestationResult_free(AVmAttestationResult* _Nullable result)
        __INTRODUCED_IN(__ANDROID_API_V__);

/**
 * Reads the DER-encoded ECPrivateKey structure specified in [RFC 5915 s3] for the
 * EC P-256 private key from the provided attestation result.
 *
 * \param result A pointer to the attestation result filled in
 *              `AVmPayload_requestAttestation` when the attestation succeeds.
 * \param data A pointer to the memory where the private key will be written
 * (can be null if size is 0).
 * \param size The maximum number of bytes that can be written to the data buffer.
 * If `size` is smaller than the total size of the private key, the key data will be
 * truncated to this `size`.
 *
 * \return The total size of the private key.
 *
 * [RFC 5915 s3]: https://datatracker.ietf.org/doc/html/rfc5915#section-3
 */
size_t AVmAttestationResult_getPrivateKey(const AVmAttestationResult* _Nonnull result,
                                          void* _Nullable data, size_t size)
        __INTRODUCED_IN(__ANDROID_API_V__);

/**
 * Signs the given message using ECDSA P-256, the message is first hashed with SHA-256 and
 * then it is signed with the attested EC P-256 private key in the attestation result.
 *
 * \param result A pointer to the attestation result filled in
 *              `AVmPayload_requestAttestation` when the attestation succeeds.
 * \param message A pointer to the message buffer.
 * \param message_size size of the message.
 * \param data A pointer to the memory where the signature will be written
 * (can be null if size is 0). The signature is a DER-encoded ECDSASignature structure
 * detailed in the [RFC 6979].
 * \param size The maximum number of bytes that can be written to the data buffer.
 * If `size` is smaller than the total size of the signature, the signature will be
 * truncated to this `size`.
 *
 * \return The size of the signature, or the size needed if the supplied buffer is too small.
 *
 * [RFC 6979]: https://datatracker.ietf.org/doc/html/rfc6979
 */
size_t AVmAttestationResult_sign(const AVmAttestationResult* _Nonnull result,
                                 const void* _Nonnull message, size_t message_size,
                                 void* _Nullable data, size_t size)
        __INTRODUCED_IN(__ANDROID_API_V__);

/**
 * Gets the number of certificates in the certificate chain.
 *
 * The certificate chain consists of a sequence of DER-encoded X.509 certificates that form
 * the attestation key's certificate chain. It starts with a leaf certificate covering the attested
 * public key and ends with a root certificate.
 *
 * \param result A pointer to the attestation result obtained from `AVmPayload_requestAttestation`
 *               when the attestation succeeds.
 *
 * \return The number of certificates in the certificate chain.
 */
size_t AVmAttestationResult_getCertificateCount(const AVmAttestationResult* _Nonnull result)
        __INTRODUCED_IN(__ANDROID_API_V__);

/**
 * Retrieves the certificate at the given `index` from the certificate chain in the provided
 * attestation result.
 *
 * The certificate chain consists of a sequence of DER-encoded X.509 certificates that form
 * the attestation key's certificate chain. It starts with a leaf certificate covering the attested
 * public key and ends with a root certificate.
 *
 * \param result A pointer to the attestation result obtained from `AVmPayload_requestAttestation`
 *               when the attestation succeeds.
 * \param index Index of the certificate to retrieve. The `index` must be within the range of
 *              [0, number of certificates). The number of certificates can be obtained with
 *              `AVmAttestationResult_getCertificateCount`.
 * \param data A pointer to the memory where the certificate will be written
 *             (can be null if size is 0).
 * \param size The maximum number of bytes that can be written to the data buffer. If `size`
 *             is smaller than the total size of the certificate, the certificate will be
 *             truncated to this `size`.
 *
 * \return The total size of the certificate at the given `index`.
 */
size_t AVmAttestationResult_getCertificateAt(const AVmAttestationResult* _Nonnull result,
                                             size_t index, void* _Nullable data, size_t size)
        __INTRODUCED_IN(__ANDROID_API_V__);

__END_DECLS
