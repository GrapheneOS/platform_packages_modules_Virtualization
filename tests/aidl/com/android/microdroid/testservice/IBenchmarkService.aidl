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

package com.android.microdroid.testservice;

/** {@hide} */
interface IBenchmarkService {
    const int PORT = 5677;

    /**
     * Measures the read rate for reading the given file.
     *
     * @return The read rate in MB/s.
     */
    double measureReadRate(String filename, boolean isRand);

    /**
     * Measures the write rate for writing the given file, creating the file if necessary
     *
     * @return The write rate in MB/s.
     */
    double measureWriteRate(String filename, long size_bytes);

    /** Returns an entry from /proc/meminfo. */
    long getMemInfoEntry(String name);

    /** Allocates anonymous memory and returns the raw pointer. */
    long allocAnonMemory(long mb);

    /**
     * Initializes the vsock server on VM.
     * @return the server socket file descriptor.
     */
    int initVsockServer(int port);

    /** Runs the vsock server on VM and receives data. */
    void runVsockServerAndReceiveData(int serverFd, int numBytesToReceive);
}
