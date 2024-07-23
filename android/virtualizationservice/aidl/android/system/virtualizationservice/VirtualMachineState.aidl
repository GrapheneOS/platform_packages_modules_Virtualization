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

/**
 * The lifecycle state of a VM.
 */
@Backing(type="int")
enum VirtualMachineState {
    /**
     * The VM has been created but not yet started.
     */
    NOT_STARTED = 0,
    /**
     * The VM is running, but the payload has not yet started.
     */
    STARTING = 1,
    /**
     * The VM is running and the payload has been started, but it has not yet indicated that it is
     * ready.
     */
    STARTED = 2,
    /**
     * The VM payload has indicated that it is ready to serve requests.
     */
    READY = 3,
    /**
     * The VM payload has finished but the VM itself is still running.
     */
    FINISHED = 4,
    /**
     * The VM has died.
     */
    DEAD = 6,
}
