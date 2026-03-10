/*
 * Copyright (C) 2025 The Android Open Source Project
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

package com.android.compos;

import com.android.compos.IVerifiedDex2OatTaskCallback;

interface IVerifiedDex2OatService {
    /**
     * Initializes system properties. ART expects interesting properties that have to be passed from
     * Android. The API client should call this method once with all desired properties, since once
     * the call completes, the service is considered initialized and cannot be re-initialized again.
     *
     * <p>If the initialization failed, Microdroid may already have some properties set. It is up to
     * the service to reject further calls by the client.
     *
     * <p>The service may reject unrecognized names, but it does not interpret values.
     */
    void initializeSystemProperties(in String[] names, in String[] values);

    /**
     * Returns the current VM's signing key, as an Ed25519 public key
     * (https://datatracker.ietf.org/doc/html/rfc8032#section-5.1.5).
     */
    byte[] getPublicKey();

    @RustDerive(Clone=true)
    parcelable FileDetails {
        /**
         * The fd number authfs will use to mount the file. The expected path of the file will be
         *  authfs_mountpoint/fd
         */
        int fd;

        /**
         * Whether or not the file is exposed as a read only(false)
         * or read write(true) file by the client.
         *
         * When {@code isRw} is {@code false} then the file is read-only,
         * and {@code verityDigest} can be non-empty.
         * verifiedDex2Oat doesn't care about the contents of input-files
         * since the safeness of these read-only input files will be
         * determined during verification.
         *
         * When {@code isRw} is {@code true} then the file should be
         * empty initially and {@code verityDigest} should also be empty.
         */
        boolean isRw;

        /* Only meaningful if {@code isRw} is false (the file is read only).
         * The fs-verity digest of the file. The format is the text
         * representation of the digest prefixed with the hashing algorithm.
         * For example when the hashing algorithm is sha256, verityDigest
         * would be:
         * sha256-9828cd65f4744d6adda216d3a63d8205375be485bfa261b3b8153d3358f5a576
         *
         * If {@code isRw} is {@code false} an empty verityDigest means
         * the verity digest is not available for the file.
         *
         * If {@code isRw} is {@code true} then an empty verityDigest is expected
         * since fs-verity enabled files should be read-only.
         */
        String verityDigest;
    }

    /** Arguments to run verifiedDex2Oat */
    @RustDerive(Clone=true)
    parcelable Dex2OatArg {
        /**
         * A format string where each occurrence of {@code !} is substituted
         * with a file descriptor from the {@code fds} array.
         *
         * <p><b>Formatting Rules:</b></p>
         * <ul>
         * <li> {@code !} is escapable, for example: {@code \!}
         * <li>Escape characters ({@code \}) can themselves be escaped (e.g., {@code \\}).</li>
         * <li> The substitution of {@code !} is positional. In other words the first {@code !}
         * is substituted with the first fd, the second with the second fd etc..
         * </li>
         * </ul>
         *
         * <p>It is an error if the count of non-escaped {@code !} placeholders does not match the
         * number of file descriptors provided in {@code fds}.</p>
         */
        String formatString;
        /*
         * An optional list of fds. The number of fds must match the number of
         * substitution {} atoms in formatString, if they do not match then
         * verifiedDex2Oat will fail.
         */
        FileDetails[] fds;
    }

    /**
     * Executes the dex2oat compiler with the specified arguments within the secure context of a
     * protected virtual machine (pVM).
     *
     * <p>This method ensures that the entire compilation process is securely managed. All compiler
     * arguments are logged to the provided {@code manifestFd}. Additionally, the fs-verity root
     * digests for all input and output files are recorded in the manifest. The pVM then signs
     * this manifest with its DICE key, which is cryptographically tied to the pVM's identity.</p>
     *
     * <p>The result is a verifiable record of the compilation, which can be used to confirm the
     * integrity of the dex2oat output. This allows for the detection of compilation artifacts that
     * may have been generated with altered compiler arguments or within an untrusted pVM.</p>
     *
     * <p>Note that the primary goal of {@code verifiedDex2Oat} is not to prevent unsafe
     * compilations, but to ensure that any such compilations are detectable.</p>
     *
     * @param args An array of {@link Dex2OatArg} objects, each specifying the format string and
     * file descriptors for the dex2oat process.
     * @systemDirFd a file descriptor number that refers to /system/.
     * @systemDirFd a file descriptor number that refers to /system_ext/ if it exists, if not
     *  this should be set to -1.
     * @param manifestFd The file descriptor for the manifest where the compilation details and
     * file fs-verity digests will be recorded.
     * @param callback An {@link IVerifiedDex2OatTaskCallback} instance to handle callbacks
     * related to the task's execution.
     */
    void verifiedDex2Oat(in Dex2OatArg[] args, int systemDirFd, int systemExtDirFd, int manifestFd,
            IVerifiedDex2OatTaskCallback callback);

    /**
     * Request the service to exit, triggering the termination of the VM. This may cause any
     * requests in flight to fail.
     */
    oneway void quit();
}
