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
import android.system.composd.IDex2OatTaskCallback;

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
    ICompilationTask startStagedApexCompile(ICompilationTaskCallback callback, String os);

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
    ICompilationTask startTestCompile(
            ApexSource apexSource, ICompilationTaskCallback callback, String os);

    /** Arguments to startVerifiedDex2Oat */
    parcelable Dex2OatArg {
        /**
         * A format string where each occurrence of {@code !} is substituted
         * with a file descriptor from the {@code fds} array.
         *
         * <p><b>Formatting Rules:</b></p>
         * <ul>
         * <li> Escaping {@code !} is supported, e.g.
         * {@code Hello=\!} will not have the {@code !} substituted with a file descriptor.<li>
         * </ul>
         *
         * <p>It is an error if the count of non-escaped {@code !} placeholders does not match the
         * number of file descriptors provided in {@code fds}.</p>
         */
        String formatString;
        /**
         * An array of file descriptors whose length must match the number of
         * {@code !} substitution placeholders in {@code formatString}.
         * A mismatch length is considered an invalid argument and will result in a failure.
         */
        ParcelFileDescriptor[] fds;
    }

    /*
     * Start a verified dex2oat operation with a timeout.
     *
     * This enqueues a dex2oat operation that will ultimately run inside of a PVM.
     * These dex2oat operations are serialized.
     *
     * {@param} args - arguments that will be passed to the dex2oat command line.
     * {@param} args_record_fd- the file descriptor where the dex2oat arguments will be recorded.
     * {@param} result_callback - The callback used to communicate the results.
     * {@param} timeout_seconds - Timeout for the compilation.
     * {@return} A compilationTask which can be used to cancel a dex2oat task. A task that is
     * cancelled may or may not call the IDex2OatTaskCallback.
     */
    ICompilationTask startVerifiedDex2Oat(in Dex2OatArg[] args,
            in ParcelFileDescriptor args_record_fd, in IDex2OatTaskCallback result_callback,
            int timeout_seconds);
}
