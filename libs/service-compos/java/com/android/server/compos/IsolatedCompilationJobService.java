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

package com.android.server.compos;

import static java.util.Objects.requireNonNull;

import android.app.job.JobInfo;
import android.app.job.JobParameters;
import android.app.job.JobScheduler;
import android.app.job.JobService;
import android.content.ComponentName;
import android.os.IBinder;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.system.composd.ICompilationTask;
import android.system.composd.ICompilationTaskCallback;
import android.system.composd.IIsolatedCompilationService;
import android.util.Log;

import com.android.server.compos.IsolatedCompilationMetrics.CompilationResult;

import java.util.NoSuchElementException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/**
 * A job scheduler service responsible for performing Isolated Compilation when scheduled.
 *
 * @hide
 */
public class IsolatedCompilationJobService extends JobService {
    private static final String TAG = IsolatedCompilationJobService.class.getName();
    private static final int STAGED_APEX_JOB_ID = 5132251;

    private final AtomicReference<CompilationJob> mCurrentJob = new AtomicReference<>();

    static void scheduleStagedApexJob(JobScheduler scheduler) {
        ComponentName serviceName =
                new ComponentName("android", IsolatedCompilationJobService.class.getName());

        int result =
                scheduler.schedule(
                        new JobInfo.Builder(STAGED_APEX_JOB_ID, serviceName)
                                // Wait in case more APEXes are staged
                                .setMinimumLatency(TimeUnit.MINUTES.toMillis(60))
                                // We consume CPU, power, and storage
                                .setRequiresDeviceIdle(true)
                                .setRequiresCharging(true)
                                .setRequiresStorageNotLow(true)
                                .build());
        if (result == JobScheduler.RESULT_SUCCESS) {
            IsolatedCompilationMetrics.onCompilationScheduled(
                    IsolatedCompilationMetrics.SCHEDULING_SUCCESS);
        } else {
            IsolatedCompilationMetrics.onCompilationScheduled(
                    IsolatedCompilationMetrics.SCHEDULING_FAILURE);
            Log.e(TAG, "Failed to schedule staged APEX job");
        }
    }

    static boolean isStagedApexJobScheduled(JobScheduler scheduler) {
        return scheduler.getPendingJob(STAGED_APEX_JOB_ID) != null;
    }

    @Override
    public boolean onStartJob(JobParameters params) {
        Log.i(TAG, "Starting job");

        // This function (and onStopJob) are only ever called on the main thread, so we don't have
        // to worry about two starts at once, or start and stop happening at once. But onCompletion
        // can be called on any thread, so we need to be careful with that.

        CompilationJob oldJob = mCurrentJob.get();
        if (oldJob != null) {
            // We're already running a job, give up on this one
            Log.w(TAG, "Another job is in progress, skipping");
            return false; // Already finished
        }

        IsolatedCompilationMetrics metrics = new IsolatedCompilationMetrics();

        CompilationJob newJob =
                new CompilationJob(
                        IsolatedCompilationJobService.this::onCompletion, params, metrics);
        mCurrentJob.set(newJob);

        // This can take some time - we need to start up a VM - so we do it on a separate
        // thread. This thread exits as soon as the compilation Task has been started (or
        // there's a failure), and then compilation continues in composd and the VM.
        new Thread("IsolatedCompilationJob_starter") {
            @Override
            public void run() {
                try {
                    newJob.start();
                } catch (RuntimeException e) {
                    Log.e(TAG, "Starting CompilationJob failed", e);
                    metrics.onCompilationEnded(IsolatedCompilationMetrics.RESULT_FAILED_TO_START);
                    mCurrentJob.set(null);
                    newJob.stop(); // Just in case it managed to start before failure
                    jobFinished(params, /* wantReschedule= */ false);
                }
            }
        }.start();
        return true; // Job is running in the background
    }

    @Override
    public boolean onStopJob(JobParameters params) {
        CompilationJob job = mCurrentJob.getAndSet(null);
        if (job == null) {
            return false; // No need to reschedule, we'd finished
        } else {
            job.stop();
            return true; // We didn't get to finish, please re-schedule
        }
    }

    void onCompletion(JobParameters params, boolean succeeded) {
        Log.i(TAG, "onCompletion, succeeded=" + succeeded);

        CompilationJob job = mCurrentJob.getAndSet(null);
        if (job == null) {
            // No need to call jobFinished if we've been told to stop.
            return;
        }
        // On success we don't need to reschedule.
        // On failure we could reschedule, but that could just use a lot of resources and still
        // fail; instead we just let odsign do compilation on reboot if necessary.
        jobFinished(params, /* wantReschedule= */ false);
    }

    interface CompilationCallback {
        void onCompletion(JobParameters params, boolean succeeded);
    }

    static class CompilationJob extends ICompilationTaskCallback.Stub
            implements IBinder.DeathRecipient {
        private final IsolatedCompilationMetrics mMetrics;
        private final AtomicReference<ICompilationTask> mTask = new AtomicReference<>();
        private final CompilationCallback mCallback;
        private final JobParameters mParams;
        private volatile boolean mStopRequested = false;

        CompilationJob(
                CompilationCallback callback,
                JobParameters params,
                IsolatedCompilationMetrics metrics) {
            mCallback = requireNonNull(callback);
            mParams = params;
            mMetrics = requireNonNull(metrics);
        }

        void start() {
            IBinder binder = ServiceManager.waitForService("android.system.composd");
            IIsolatedCompilationService composd =
                    IIsolatedCompilationService.Stub.asInterface(binder);

            if (composd == null) {
                throw new IllegalStateException("Unable to find composd service");
            }

            try {
                ICompilationTask composTask = composd.startStagedApexCompile(this);
                mMetrics.onCompilationStarted();
                mTask.set(composTask);
                composTask.asBinder().linkToDeath(this, 0);
            } catch (RemoteException e) {
                throw e.rethrowAsRuntimeException();
            }

            if (mStopRequested) {
                // We were asked to stop while we were starting the task. We need to
                // cancel it now, since we couldn't before.
                cancelTask();
            }
        }

        void stop() {
            mStopRequested = true;
            cancelTask();
        }

        private void cancelTask() {
            ICompilationTask task = mTask.getAndSet(null);
            if (task == null) {
                return;
            }

            Log.i(TAG, "Cancelling task");
            try {
                task.cancel();
            } catch (RuntimeException | RemoteException e) {
                // If canceling failed we'll assume it means that the task has already failed;
                // there's nothing else we can do anyway.
                Log.w(TAG, "Failed to cancel CompilationTask", e);
            }

            mMetrics.onCompilationJobCanceled(mParams.getStopReason());
            try {
                task.asBinder().unlinkToDeath(this, 0);
            } catch (NoSuchElementException e) {
                // Harmless
            }
        }

        @Override
        public void binderDied() {
            onCompletion(false, IsolatedCompilationMetrics.RESULT_COMPOSD_DIED);
        }

        @Override
        public void onSuccess() {
            onCompletion(true, IsolatedCompilationMetrics.RESULT_SUCCESS);
        }

        @Override
        public void onFailure(byte reason, String message) {
            int result;
            switch (reason) {
                case ICompilationTaskCallback.FailureReason.CompilationFailed:
                    result = IsolatedCompilationMetrics.RESULT_COMPILATION_FAILED;
                    break;

                case ICompilationTaskCallback.FailureReason.UnexpectedCompilationResult:
                    result = IsolatedCompilationMetrics.RESULT_UNEXPECTED_COMPILATION_RESULT;
                    break;

                case ICompilationTaskCallback.FailureReason.FailedToEnableFsverity:
                    result = IsolatedCompilationMetrics.RESULT_FAILED_TO_ENABLE_FSVERITY;
                    break;

                default:
                    result = IsolatedCompilationMetrics.RESULT_UNKNOWN_FAILURE;
                    break;
            }
            Log.w(TAG, "Compilation failed: " + message);
            onCompletion(false, result);
        }

        private void onCompletion(boolean succeeded, @CompilationResult int result) {
            ICompilationTask task = mTask.getAndSet(null);
            if (task != null) {
                mMetrics.onCompilationEnded(result);
                mCallback.onCompletion(mParams, succeeded);
                try {
                    task.asBinder().unlinkToDeath(this, 0);
                } catch (NoSuchElementException e) {
                    // Harmless
                }
            }
        }
    }
}
