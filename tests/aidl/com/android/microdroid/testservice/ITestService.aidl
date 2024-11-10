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
package com.android.microdroid.testservice;

import com.android.microdroid.testservice.IAppCallback;

/**
 * This is the service exposed by the test payload, called by the test app.
 * {@hide}
 */
interface ITestService {
    const long PORT = 5678;

    const long ECHO_REVERSE_PORT = 0x80000001L; // Deliberately chosen to be > 2^31, < 2^32

    /* add two integers. */
    int addInteger(int a, int b);

    /* read a system property. */
    String readProperty(String prop);

    /* get a VM instance secret, this is _only_ done for testing. */
    byte[] insecurelyExposeVmInstanceSecret();

    /* get the VM's attestation secret, this is _only_ done for testing. */
    byte[] insecurelyExposeAttestationCdi();

    /* get the VM's boot certificate chain (BCC). */
    byte[] getBcc();

    /* get the APK contents path. */
    String getApkContentsPath();

    /* get the encrypted storage path. */
    String getEncryptedStoragePath();

    /* start a simple vsock server on ECHO_REVERSE_PORT that reads a line at a time and echoes
     * each line reverse.
     */
    void runEchoReverseServer();

    /** Returns a mask of effective capabilities that the process running the payload binary has. */
    String[] getEffectiveCapabilities();

    /* Return the uid of the process running the binary. */
    int getUid();

    /* write the content into the specified file. */
    void writeToFile(String content, String path);

    /* get the content of the specified file. */
    String readFromFile(String path);

    /* get file permissions of the give file by stat'ing it */
    int getFilePermissions(String path);

    /** Returns flags for the given mountPoint. */
    int getMountFlags(String mountPoint);

    /** Returns page size of the VM. */
    int getPageSize();

    /** Requests the VM to asynchronously call appCallback.setVmCallback() */
    void requestCallback(IAppCallback appCallback);

    /** Read a line from /dev/console */
    String readLineFromConsole();

    /**
     * Request the service to exit, triggering the termination of the VM. This may cause any
     * requests in flight to fail.
     */
    oneway void quit();
}
