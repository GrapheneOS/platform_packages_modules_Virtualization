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
     * On a successful compilation this function is called.
     *
     * {@param} metrics pertaining to the successful compilation.
     */
    void onSuccess(in GuestDex2OatMetrics metrics);

    /**
     * Failures where dex2oat returned an exit code.
     */
    @RustDerive(PartialEq=true, Clone=true)
    parcelable Dex2OatExitCode {
        int exit_code;
        GuestDex2OatMetrics metrics;
    }
    /**
     * Failures where dex2oat was terminated by a signal.
     */
    @RustDerive(PartialEq=true, Clone=true)
    parcelable Dex2OatSignal {
        int signal;
        GuestDex2OatMetrics metrics;
    }
    /**
     * Failures that occurred while preparing to run dex2oat.
     */
    @RustDerive(PartialEq=true, Clone=true)
    parcelable Dex2OatSetupFailure {
        String message;
        int[] relevant_fds;
    }
    /**
     * The details of why a verifiedDex2Oat failed.
     */
    @RustDerive(PartialEq=true, Clone=true)
    union GuestFailureDetails {
        Dex2OatExitCode exit_code;
        Dex2OatSignal signal;
        Dex2OatSetupFailure setup;
    }
    /**
     * On a failed compilation this function is called.
     *
     * {@param} failureDetails contains the details of why verifiedDex2Oat failed.
     */
    void onFailure(in GuestFailureDetails failureDetails);
}
