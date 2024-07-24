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
package android.system.composd;

/**
 * Interface to be implemented by clients of IIsolatedCompilationService to be notified when a
 * requested compilation task completes.
 */
oneway interface ICompilationTaskCallback {
    enum FailureReason {
        /** We failed to successfully start the VM and run compilation in it. */
        CompilationFailed,
        /** We ran compilation in the VM, but it reported a problem. */
        UnexpectedCompilationResult,
        /** We failed to enable fs-verity completely to the output artifacts. */
        FailedToEnableFsverity,
    }

    /**
     * Called if a compilation task has ended successfully, generating all the required artifacts.
     */
    void onSuccess();

    /**
     * Called if a compilation task has ended unsuccessfully.
     */
    void onFailure(FailureReason reason, String message);
}
