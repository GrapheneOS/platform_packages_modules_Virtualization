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
     * Port for VM dump service.
     * TODO(b/423899247): add dump() method instead of raw vsock, once the global virtualization
     * service can connect to the binder service of virtmgr / VMs.
     */
    const int DUMP_SERVICE_PORT = 100000;

    /**
     * Shuts the VM down gracefully.
     */
    oneway void shutdown();

    /** Requests the VM to trim its memory usage. */
    oneway void trim();
}
