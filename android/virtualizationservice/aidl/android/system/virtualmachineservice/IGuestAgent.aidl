/*
 * Copyright 2025 The Android Open Source Project
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

/** {@hide} */
interface IGuestAgent {
    /**
     * Starts a vsock server to dump the VM's state, and return a port number for the listening
     * vsock. The guest agent must open a vsock server which accepts one client, and then sends VM's
     * dump to the client. Writing to the client vsock must be done within 5 seconds. Otherwise, the
     * requester may regard it as a timeout.
     *
     * TODO(b/395205629): Use IBinder::Interface::dump().
     */
    int startDumpVsockServer(in String[] args);

    /**
     * Shuts the VM down gracefully.
     */
    @SuppressWarnings(value={"mixed-oneway"}) oneway void shutdownAsync();

    /** Requests the VM to trim its memory usage. */
    @SuppressWarnings(value={"mixed-oneway"}) oneway void trimAsync();
}
