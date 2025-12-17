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
package android.system.composd;

/**
 * Interface to be implemented by clients of IIsolatedCompilationService to be notified when a
 * requested compilation task completes.
 */
oneway interface IDex2OatTaskCallback {
    enum FailureReason {
        /**
         * We failed to successfully start the VM and prepare the environment for compilation.
         */
        CompilationSetupFailed,
        /**
         * We ran dex2oat in the VM and it returned an error.
         */
        Dex2OatFailed,
        /**
         * We failed to enable fs-verity completely to the output artifacts.
         */
        FailedToEnableFsVerity,
        /**
         * Compilation could not be completed within specified timeout.
         */
        Timeout,
    }
    /**
     * The details of why a startVerifiedDex2Oat failed.
     */
    @RustDerive(PartialEq=true, Clone=true)
    parcelable FailureDetails {
        /**
         * The reason why the compilation failed.
         */
        FailureReason reason;
        /**
         * The exit code of dex2oat, only meaningful when reason=Dex2OatFailed.
         */
        int exit_code;
        /**
         * If the compilation failed due to a signal this will be set to the
         * POSIX signal code, otherwise it is set to 0.
         */
        int signal;
        /**
         * The total amount of time between dex2oat is invoked within the PVM
         * until the compilation failed.
         */
        int wallclock_time_milliseconds;
        /**
         * The total amount of time dex2oat was actively compiling within the PVM
         * before failure.
         */
        int cpu_time_milliseconds;
        /**
         * Additional description of the failure.
         */
        String message;
    }

    @RustDerive(PartialEq=true, Clone=true, Copy=true)
    parcelable Dex2OatMetrics {
        /**
         * The total amount of time the compilation took from the time
         * dex2oat was called within the PVM until the compilation finished.
         */
        int wallclock_time_milliseconds;
        /**
         * The total amount of time dex2oat was actively compiling within the PVM.
         */
        int cpu_time_milliseconds;
    }

    /**
     * Called if a compilation task has ended successfully, generating all the required artifacts.
     */
    void onSuccess(in Dex2OatMetrics metrics);

    /**
     * Called if a compilation task has ended unsuccessfully.
     */
    void onFailure(in FailureDetails failure_details);
}
