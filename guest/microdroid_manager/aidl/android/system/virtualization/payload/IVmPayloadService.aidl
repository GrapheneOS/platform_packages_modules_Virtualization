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

package android.system.virtualization.payload;

import android.os.ParcelFileDescriptor;
import android.system.virtualizationcommon.Certificate;

/**
 * This interface regroups the tasks that payloads delegate to
 * Microdroid Manager for execution.
 */
interface IVmPayloadService {
    /** The constants STATUS_* are status code returned by this service. */
    /** Failed to prepare the CSR and key pair for attestation. */
    const int STATUS_FAILED_TO_PREPARE_CSR_AND_KEY = 1;

    /** Socket name of the service IVmPayloadService. */
    const String VM_PAYLOAD_SERVICE_SOCKET_NAME = "vm_payload_service";

    /** Path to the APK contents path. */
    const String VM_APK_CONTENTS_PATH = "/mnt/apk";

    /**
     * Path to the encrypted storage. Note the path will not exist if encrypted storage
     * is not enabled.
     */
    const String ENCRYPTEDSTORE_MOUNTPOINT = "/mnt/encryptedstore";

    /**
     * Path to the per user credential encrypted folder location
     */
    const String ENCRYPTEDSTORE_PER_USER_FOLDERS = "/mnt/encryptedstore/users";

    /**
     * Path to the microdroid managed socket directory.
     */
    const String MICRODROID_SOCKET_PATH = "/dev/socket/microdroid_managed";

    /**
     * An {@link AttestationResult} holds an attested private key and the remotely
     * provisioned certificate chain covering its corresponding public key.
     */
    parcelable AttestationResult {
        /**
         * DER-encoded ECPrivateKey structure specified in [RFC 5915 s3] for the
         * EC P-256 private key, which is attested.
         *
         * The corresponding public key is included in the leaf certificate of
         * the certificate chain.
         *
         * [RFC 5915 s3]: https://datatracker.ietf.org/doc/html/rfc5915#section-3
         */
        byte[] privateKey;

        /**
         * Sequence of DER-encoded X.509 certificates that make up the attestation
         * key's certificate chain.
         *
         * The certificate chain starts with a leaf certificate covering the attested
         * public key and ends with a root certificate.
         */
        Certificate[] certificateChain;
    }

    /** Notifies that the payload is ready to serve. */
    void notifyPayloadReady();

    /**
     * Gets a secret that is uniquely bound to this VM instance.
     *
     * @param identifier the identifier of the secret to return.
     * @param size the number of bytes of the secret to return.
     * @return size bytes of the identified secret.
     */
    byte[] getVmInstanceSecret(in byte[] identifier, int size);

    /**
     * Write `data`, on behalf of the client, to Secretkeeper.
     * This is confidential to the pVM and protected via appropriate DICE policy
     * on the payload's DICE chain.
     */
    void writePayloadRpData(in byte[32] data);

    /**
     * Read payload's `data` written on behalf of the payload in Secretkeeper.
     * The returned value can be null either due to no value written or because
     * Android maliciously deleted the value - Secretkeeper deletion are not authenticated.
     */
    @nullable byte[32] readPayloadRpData();

    /**
     * Gets the DICE attestation chain for the VM.
     *
     * The DICE chain must not be made available to all VMs as it contains privacy breaking
     * identifiers.
     *
     * @return the VM's raw DICE certificate chain.
     * @throws SecurityException if the use of test APIs is not permitted.
     */
    byte[] getDiceAttestationChain();

    /**
     * Gets the DICE attestation CDI for the VM.
     *
     * The raw attestation CDI isn't very useful but is used for smoke tests. A better API would
     * handle key derivation on behalf of the payload so they can't forget to do it themselves and
     * would also mean the payload doesn't get the raw CDI which reduces the chance of it leaking.
     *
     * @return the VM's raw attestation CDI.
     * @throws SecurityException if the use of test APIs is not permitted.
     */
    byte[] getDiceAttestationCdi();

    /**
     * Requests the remote attestation of the client VM.
     *
     * The challenge will be included in the certificate chain in the attestation result,
     * serving as proof of the freshness of the result.
     *
     * @param challenge the maximum supported challenge size is 64 bytes.
     * @param testMode whether the attestation is only for testing purposes. If testMode is true,
     * caller must invoke {@link VirtualMachineManager#enableTestAttestation} prior to
     * calling this method to provision a key pair to sign the attested result, and the returned
     * certificate chain will not be RKP server rooted.
     *
     * @return An {@link AttestationResult} parcelable containing an attested key pair and its
     *         certification chain.
     */
    AttestationResult requestAttestation(in byte[] challenge, in boolean testMode);

    /**
     * Checks whether the VM instance is new - i.e., if this is the first run of an instance.
     * This is an indication of fresh new VM secrets. Payload can use this to setup the fresh
     * instance if needed.
     *
     * @return true on the first boot of the instance & false on subsequent boot.
     */
    boolean isNewInstance();

    /**
     * Creates a dm-crypt mapping over an encrypted image file and mounts the contained filesystem.
     *
     * This function allows a payload to mount a read-only filesystem image that is encrypted.
     * The image must be located within the APK's assets directory.
     * The filesystem is mounted read-only and executable.
     *
     * @param imagePath Absolute path to the encrypted filesystem image file.
     *                  Must be under the APK assets directory.
     * @param fsType The filesystem type of the image (e.g., "erofs").
     * @param cipher The encryption cipher to use for dm-crypt.
     *               Supported values are "aes-xts-plain64" and "aes-hctr2-plain64".
     * @param key Raw encryption key bytes.
     * @param sectorSize The logical sector size of the block device in bytes.
     *                   Must be a power of two between 512 and 4096.
     *
     * @return The absolute path to the mount point on success.
     * @throws IllegalArgumentException if any of the arguments are invalid (e.g., bad path,
     *                                  unsupported fsType or cipher, invalid sectorSize).
     * @throws ServiceSpecificException for any other internal failures during the mount process.
     */
    @utf8InCpp String mountEncryptedAssets(in @utf8InCpp String imagePath,
            in @utf8InCpp String fsType, in @utf8InCpp String cipher, in byte[] key,
            in int sectorSize);

    /**
     * Returns the absolute path to the encrypted storage path for the given tenant.
     *
     * @param uid The uid of the tenant.
     * @return The path to the encrypted storage.
     */
    String getEncryptedStoragePath(in long uid);

    /**
     * Creates a unix domain socket with the given name at the path /dev/socket/.
     *
     * @param name the name of the unix domain socket.
     * @return the file descriptor of the unix domain socket.
     */
    ParcelFileDescriptor createUnixDomainSocket(in @utf8InCpp String name);

    /**
     * Returns the absolute path to the APK contents path for the given tenant.
     *
     * @param uid The uid of the tenant.
     * @return The path to the APK contents.
     */
    String getApkContentsPath(in long uid);
}
