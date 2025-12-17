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
package com.android.compos;

/**
 * Interface implemented by ICompOsService clients to be notified when a
 * verified dex2oat task completes.
 */
oneway interface IVerifiedDex2OatTaskCallback {
    /**
     * Called if a compilation successfully completes, generating all the required artifacts.
     * cpuTimeMs is the cpu time (in milliseconds) of dex2oat during compilation within the
     * pVM.
     *
     * wallTimeMs is the amount of total elapsed time from the start of invoking dex2oat
     * in the pVM until it exits.
     */
    void onSuccess(int cpuTimeMs, int wallTimeMs, byte exitCode);

    /**
     * Called if a verified dex2oat task has failed.
     *
     * message is a descriptive message of the failure.
     *
     * exitCode is the exit code of the dex2oat ran within the pVM.
     */
    void onFailure(String message, byte exitCode, int cpuTimeMs, int WallTimeMs);
}
