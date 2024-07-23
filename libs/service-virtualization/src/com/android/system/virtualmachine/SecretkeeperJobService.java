/*
 * Copyright (C) 2024 The Android Open Source Project
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

package com.android.system.virtualmachine;

import static android.content.pm.PackageManager.MATCH_UNINSTALLED_PACKAGES;

import android.app.job.JobInfo;
import android.app.job.JobParameters;
import android.app.job.JobScheduler;
import android.app.job.JobService;
import android.content.ComponentName;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.content.pm.PackageManager.ApplicationInfoFlags;
import android.os.RemoteException;
import android.os.ServiceSpecificException;
import android.os.UserHandle;
import android.system.virtualizationmaintenance.IVirtualizationMaintenance;
import android.system.virtualizationmaintenance.IVirtualizationReconciliationCallback;
import android.util.Log;

import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;

import java.util.Arrays;
import java.util.List;
import java.util.concurrent.atomic.AtomicReference;

/**
 * A job scheduler service responsible for triggering the Virtualization Service reconciliation
 * process when scheduled. The job is scheduled to run once per day while idle and charging.
 *
 * <p>The reconciliation process ensures that Secretkeeper secrets belonging to apps or users that
 * have been removed get deleted.
 *
 * @hide
 */
public class SecretkeeperJobService extends JobService {
    private static final String TAG = SecretkeeperJobService.class.getName();
    private static final String JOBSCHEDULER_NAMESPACE = "VirtualizationSystemService";
    private static final int JOB_ID = 1;
    private static final AtomicReference<SecretkeeperJob> sJob = new AtomicReference<>();

    static void scheduleJob(JobScheduler scheduler) {
        try {
            ComponentName serviceName =
                    new ComponentName("android", SecretkeeperJobService.class.getName());
            scheduler = scheduler.forNamespace(JOBSCHEDULER_NAMESPACE);
            if (scheduler.schedule(
                            new JobInfo.Builder(JOB_ID, serviceName)
                                    // We consume CPU and power
                                    .setRequiresDeviceIdle(true)
                                    .setRequiresCharging(true)
                                    .setPeriodic(24 * 60 * 60 * 1000L)
                                    .build())
                    != JobScheduler.RESULT_SUCCESS) {
                Log.e(TAG, "Unable to schedule job");
                return;
            }
            Log.i(TAG, "Scheduled job");
        } catch (Exception e) {
            Log.e(TAG, "Failed to schedule job", e);
        }
    }

    @Override
    public boolean onStartJob(JobParameters params) {
        Log.i(TAG, "Starting job");

        SecretkeeperJob job = new SecretkeeperJob(getPackageManager());
        sJob.set(job);

        new Thread("SecretkeeperJob") {
            @Override
            public void run() {
                try {
                    job.run();
                    Log.i(TAG, "Job finished");
                } catch (Exception e) {
                    Log.e(TAG, "Job failed", e);
                }
                sJob.set(null);
                // We don't reschedule on error, we will try again the next day anyway.
                jobFinished(params, /* wantReschedule= */ false);
            }
        }.start();

        return true; // Job is running in the background
    }

    @Override
    public boolean onStopJob(JobParameters params) {
        Log.i(TAG, "Stopping job");
        SecretkeeperJob job = sJob.getAndSet(null);
        if (job != null) {
            job.stop();
        }
        return false; // Idle jobs get rescheduled anyway
    }

    private static class SecretkeeperJob {
        private final UserManagerInternal mUserManager =
                LocalServices.getService(UserManagerInternal.class);
        private volatile boolean mStopRequested = false;
        private PackageManager mPackageManager;

        public SecretkeeperJob(PackageManager packageManager) {
            mPackageManager = packageManager;
        }

        public void run() throws RemoteException {
            IVirtualizationMaintenance maintenance =
                    VirtualizationSystemService.connectToMaintenanceService();
            maintenance.performReconciliation(new Callback());
        }

        public void stop() {
            mStopRequested = true;
        }

        class Callback extends IVirtualizationReconciliationCallback.Stub {
            @Override
            public boolean[] doUsersExist(int[] userIds) {
                checkForStop();
                int[] currentUsers = mUserManager.getUserIds();
                boolean[] results = new boolean[userIds.length];
                for (int i = 0; i < userIds.length; i++) {
                    // The total number of users is likely to be small, so no need to make this
                    // better than O(N).
                    for (int user : currentUsers) {
                        if (user == userIds[i]) {
                            results[i] = true;
                            break;
                        }
                    }
                }
                return results;
            }

            @Override
            public boolean[] doAppsExist(int userId, int[] appIds) {
                checkForStop();

                // If an app has been uninstalled but its data is still present we want to include
                // it, since that might include a VM which will be used in the future.
                ApplicationInfoFlags flags = ApplicationInfoFlags.of(MATCH_UNINSTALLED_PACKAGES);
                List<ApplicationInfo> appInfos =
                        mPackageManager.getInstalledApplicationsAsUser(flags, userId);
                int[] currentAppIds = new int[appInfos.size()];
                for (int i = 0; i < appInfos.size(); i++) {
                    currentAppIds[i] = UserHandle.getAppId(appInfos.get(i).uid);
                }
                Arrays.sort(currentAppIds);

                boolean[] results = new boolean[appIds.length];
                for (int i = 0; i < appIds.length; i++) {
                    results[i] = Arrays.binarySearch(currentAppIds, appIds[i]) >= 0;
                }

                return results;
            }

            private void checkForStop() {
                if (mStopRequested) {
                    throw new ServiceSpecificException(ERROR_STOP_REQUESTED, "Stop requested");
                }
            }
        }
    }
}
