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
package android.system.virtualizationservice;

import android.system.virtualizationservice.VirtualMachineState;

/** Information about a running VM, for debug purposes only. */
parcelable VirtualMachineDebugInfo {
    /** The CID assigned to the VM. */
    int cid;

    /** Directory of temporary files used by the VM while it is running. */
    @utf8InCpp String temporaryDirectory;

    /** The UID of the process which requested the VM. */
    int requesterUid;

    /**
     * The PID of the process which requested the VM. Note that this process may no longer exist and
     * the PID may have been reused for a different process, so this should not be trusted.
     */
    int requesterPid;

    /** The peer end (ptsname) of the host console. */
    @nullable @utf8InCpp String hostConsoleName;
}
