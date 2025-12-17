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
    @RustDerive(PartialEq=true, Clone=true, Copy=true)
    parcelable GuestDex2OatMetrics {
        /**
         * The total amount of time the compilation took from the time
         * dex2oat was called until the compilation finished.
         */
        int wallclock_time_milliseconds;
        /**
         * The total amount of time dex2oat was actively compiling.
         */
        int cpu_time_milliseconds;
    }
    /**
     * Called if a compilation successfully completes, generating all the required artifacts.
     * cpuTimeMs is the cpu time (in milliseconds) of dex2oat during compilation within the
     * pVM.
     *
     * {@param} metrics pertaining to the successful compilation.
     */
    void onSuccess(in GuestDex2OatMetrics metrics);

    /**
     * The details of why a verifiedDex2Oat failed.
     */
    @RustDerive(PartialEq=true, Clone=true)
    parcelable GuestFailureDetails {
        /**
         * The exit code of dex2oat if available, otherwise this
         * is set to -1.
         */
        int exit_code;
        /**
         * If the compilation failed due to a signal this will be set to the
         * POSIX signal code, otherwise it is set to -1.
         */
        int signal;
        /**
         * The total amount of time between dex2oat is invoked until the
         * compilation failed. If dex2oat never ran this is set to -1.
         */
        int wallclock_time_milliseconds;
        /**
         * The total amount of time dex2oat was actively compiling before
         * failure.
         */
        int cpu_time_milliseconds;
        /**
         * Additional description of the failure.
         */
        String message;
    }
    /**
     * On a failed compilation this function is called.
     *
     * {@param} failureDetails contains the details of why verifiedDex2Oat failed.
     */
    void onFailure(in GuestFailureDetails failureDetails);
}
