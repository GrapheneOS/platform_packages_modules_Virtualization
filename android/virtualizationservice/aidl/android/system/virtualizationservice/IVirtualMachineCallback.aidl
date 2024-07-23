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

import android.system.virtualizationcommon.DeathReason;
import android.system.virtualizationcommon.ErrorCode;

/**
 * An object which a client may register with the VirtualizationService to get callbacks about the
 * state of a particular VM.
 */
oneway interface IVirtualMachineCallback {
    /**
     * Called when the payload starts in the VM.
     */
    void onPayloadStarted(int cid);

    /**
     * Called when the payload in the VM is ready to serve.
     */
    void onPayloadReady(int cid);

    /**
     * Called when the payload has finished in the VM. `exitCode` is the exit code of the payload.
     */
    void onPayloadFinished(int cid, int exitCode);

    /**
     * Called when an error occurs in the VM.
     */
    void onError(int cid, ErrorCode errorCode, in String message);

    /**
     * Called when the VM dies.
     *
     * Note that this will not be called if the VirtualizationService itself dies, so you should
     * also use `link_to_death` to handle that.
     */
    void onDied(int cid, in DeathReason reason);
}
