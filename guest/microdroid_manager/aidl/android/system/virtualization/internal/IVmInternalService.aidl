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

package android.system.virtualization.internal;

/**
 * This interface supports communication from system
 * components (e.g. encryptedstore) back to microdroid manager.
 */
interface IVmInternalService {
    /** Socket name of the service IVmInternalService. */
    const String VM_INTERNAL_SERVICE_SOCKET_NAME = "vm_internal_service";
    /** Report the failure fsck process to the host statsd service. */
    void reportAtomFsckFailedToHost(in int exitCode);
}
