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

package com.android.server.compos;

import android.annotation.IntDef;
import android.app.job.JobParameters;
import android.os.SystemClock;
import android.util.Log;

import com.android.internal.art.ArtStatsLog;

import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;

/**
 * A class that handles reporting metrics relating to Isolated Compilation to statsd.
 *
 * @hide
 */
class IsolatedCompilationMetrics {
    private static final String TAG = IsolatedCompilationMetrics.class.getName();

    // TODO(b/218525257): Move the definition of these enums to atoms.proto
    @Retention(RetentionPolicy.SOURCE)
    @IntDef({
        RESULT_UNKNOWN,
        RESULT_SUCCESS,
        RESULT_UNKNOWN_FAILURE,
        RESULT_FAILED_TO_START,
        RESULT_JOB_CANCELED,
        RESULT_COMPILATION_FAILED,
        RESULT_UNEXPECTED_COMPILATION_RESULT,
        RESULT_COMPOSD_DIED,
        RESULT_FAILED_TO_ENABLE_FSVERITY
    })
    public @interface CompilationResult {}

    // Keep this in sync with Result enum in IsolatedCompilationEnded in
    // frameworks/proto_logging/stats/atoms.proto
    public static final int RESULT_UNKNOWN =
            ArtStatsLog.ISOLATED_COMPILATION_ENDED__COMPILATION_RESULT__RESULT_UNKNOWN;
    public static final int RESULT_SUCCESS =
            ArtStatsLog.ISOLATED_COMPILATION_ENDED__COMPILATION_RESULT__RESULT_SUCCESS;
    public static final int RESULT_UNKNOWN_FAILURE =
            ArtStatsLog.ISOLATED_COMPILATION_ENDED__COMPILATION_RESULT__RESULT_UNKNOWN_FAILURE;
    public static final int RESULT_FAILED_TO_START =
            ArtStatsLog.ISOLATED_COMPILATION_ENDED__COMPILATION_RESULT__RESULT_FAILED_TO_START;
    public static final int RESULT_JOB_CANCELED =
            ArtStatsLog.ISOLATED_COMPILATION_ENDED__COMPILATION_RESULT__RESULT_JOB_CANCELED;
    public static final int RESULT_COMPILATION_FAILED =
            ArtStatsLog.ISOLATED_COMPILATION_ENDED__COMPILATION_RESULT__RESULT_COMPILATION_FAILED;
    public static final int RESULT_UNEXPECTED_COMPILATION_RESULT =
            ArtStatsLog
                    .ISOLATED_COMPILATION_ENDED__COMPILATION_RESULT__RESULT_UNEXPECTED_COMPILATION_RESULT;
    public static final int RESULT_COMPOSD_DIED =
            ArtStatsLog.ISOLATED_COMPILATION_ENDED__COMPILATION_RESULT__RESULT_COMPOSD_DIED;
    public static final int RESULT_FAILED_TO_ENABLE_FSVERITY =
            ArtStatsLog
                    .ISOLATED_COMPILATION_ENDED__COMPILATION_RESULT__RESULT_FAILED_TO_ENABLE_FSVERITY;

    @Retention(RetentionPolicy.SOURCE)
    @IntDef({SCHEDULING_RESULT_UNKNOWN, SCHEDULING_SUCCESS, SCHEDULING_FAILURE})
    public @interface ScheduleJobResult {}

    // Keep this in sync with Result enum in IsolatedCompilationScheduled in
    // frameworks/proto_logging/stats/atoms.proto

    public static final int SCHEDULING_RESULT_UNKNOWN =
            ArtStatsLog
                    .ISOLATED_COMPILATION_SCHEDULED__SCHEDULING_RESULT__SCHEDULING_RESULT_UNKNOWN;
    public static final int SCHEDULING_FAILURE =
            ArtStatsLog.ISOLATED_COMPILATION_SCHEDULED__SCHEDULING_RESULT__SCHEDULING_FAILURE;
    public static final int SCHEDULING_SUCCESS =
            ArtStatsLog.ISOLATED_COMPILATION_SCHEDULED__SCHEDULING_RESULT__SCHEDULING_SUCCESS;

    private long mCompilationStartTimeMs = 0;

    public static void onCompilationScheduled(@ScheduleJobResult int result) {
        ArtStatsLog.write(ArtStatsLog.ISOLATED_COMPILATION_SCHEDULED, result);
        Log.i(TAG, "ISOLATED_COMPILATION_SCHEDULED: " + result);
    }

    public void onCompilationStarted() {
        mCompilationStartTimeMs = SystemClock.elapsedRealtime();
    }

    public void onCompilationJobCanceled(@JobParameters.StopReason int jobStopReason) {
        statsLogPostCompilation(RESULT_JOB_CANCELED, jobStopReason);
    }

    public void onCompilationEnded(@CompilationResult int result) {
        statsLogPostCompilation(result, JobParameters.STOP_REASON_UNDEFINED);
    }

    private void statsLogPostCompilation(
            @CompilationResult int result, @JobParameters.StopReason int jobStopReason) {

        long compilationTime =
                mCompilationStartTimeMs == 0
                        ? -1
                        : SystemClock.elapsedRealtime() - mCompilationStartTimeMs;
        mCompilationStartTimeMs = 0;

        ArtStatsLog.write(
                ArtStatsLog.ISOLATED_COMPILATION_ENDED, compilationTime, result, jobStopReason);
        Log.i(
                TAG,
                "ISOLATED_COMPILATION_ENDED: "
                        + result
                        + ", "
                        + compilationTime
                        + ", "
                        + jobStopReason);
    }
}
