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

import android.system.composd.ICompilationTask;
import android.system.composd.ICompilationTaskCallback;

interface IIsolatedCompilationService {
    enum ApexSource {
        /** Only use the activated APEXes */
        NoStaged,
        /** Prefer any staged APEXes, otherwise use the activated ones */
        PreferStaged,
    }

    /**
     * Compile BCP extensions and system server, using any staged APEXes that are present in
     * preference to active APEXes, writing the results to the pending artifacts directory to be
     * verified by odsing on next boot.
     *
     * Compilation continues in the background, and success/failure is reported via the supplied
     * callback, unless the returned ICompilationTask is cancelled. The caller should maintain
     * a reference to the ICompilationTask until compilation completes or is cancelled.
     */
    ICompilationTask startStagedApexCompile(ICompilationTaskCallback callback);

    /**
     * Run odrefresh in a test instance of CompOS until completed or failed.
     *
     * This compiles BCP extensions and system server, even if the system artifacts are up to date,
     * and writes the results to a test directory to avoid disrupting any real artifacts in
     * existence.
     *
     * Compilation continues in the background, and success/failure is reported via the supplied
     * callback, unless the returned ICompilationTask is cancelled. The caller should maintain
     * a reference to the ICompilationTask until compilation completes or is cancelled.
     */
    ICompilationTask startTestCompile(ApexSource apexSource, ICompilationTaskCallback callback);
}
