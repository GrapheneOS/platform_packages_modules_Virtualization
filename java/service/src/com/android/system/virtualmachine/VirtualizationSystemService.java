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

import android.app.job.JobScheduler;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.net.TetheringManager;
import android.net.TetheringManager.StartTetheringCallback;
import android.net.TetheringManager.TetheringRequest;
import android.os.Handler;
import android.os.IBinder;
import android.os.ServiceManager;
import android.os.UserHandle;
import android.system.virtualizationmaintenance.IVirtualizationMaintenance;
import android.system.vmtethering.IVmTethering;
import android.util.Log;

import com.android.internal.os.BackgroundThread;
import com.android.server.SystemService;

import java.nio.file.Files;
import java.nio.file.Paths;

/**
 * This class exists to notify virtualization service of relevant things happening in the Android
 * framework.
 *
 * <p>It currently is responsible for Secretkeeper-related maintenance - ensuring that we are not
 * storing secrets for apps or users that no longer exist.
 */
public class VirtualizationSystemService extends SystemService {
    private static final String TAG = VirtualizationSystemService.class.getName();
    private static final String MAINTENANCE_SERVICE_NAME =
            "android.system.virtualizationmaintenance";
    private Handler mHandler;
    private final TetheringService mTetheringService;

    public VirtualizationSystemService(Context context) {
        super(context);
        if (Files.exists(Paths.get("/apex/com.android.virt/bin/vmnic"))) {
            mTetheringService = new TetheringService();
        } else {
            mTetheringService = null;
        }
    }

    @Override
    public void onStart() {
        if (mTetheringService != null) {
            publishBinderService(IVmTethering.DESCRIPTOR, mTetheringService);
        }
    }

    @Override
    public void onBootPhase(int phase) {
        if (phase != PHASE_BOOT_COMPLETED) return;

        mHandler = BackgroundThread.getHandler();
        new Receiver().registerForBroadcasts();

        SecretkeeperJobService.scheduleJob(getContext().getSystemService(JobScheduler.class));
    }

    private void notifyAppRemoved(int uid) {
        try {
            IVirtualizationMaintenance maintenance = connectToMaintenanceService();
            maintenance.appRemoved(UserHandle.getUserId(uid), UserHandle.getAppId(uid));
        } catch (Exception e) {
            Log.e(TAG, "notifyAppRemoved failed", e);
        }
    }

    private void notifyUserRemoved(int userId) {
        try {
            IVirtualizationMaintenance maintenance = connectToMaintenanceService();
            maintenance.userRemoved(userId);
        } catch (Exception e) {
            Log.e(TAG, "notifyUserRemoved failed", e);
        }
    }

    static IVirtualizationMaintenance connectToMaintenanceService() {
        IBinder binder = ServiceManager.waitForService(MAINTENANCE_SERVICE_NAME);
        IVirtualizationMaintenance maintenance =
                IVirtualizationMaintenance.Stub.asInterface(binder);
        if (maintenance == null) {
            throw new IllegalStateException("Failed to connect to " + MAINTENANCE_SERVICE_NAME);
        }
        return maintenance;
    }

    private class Receiver extends BroadcastReceiver {
        public void registerForBroadcasts() {
            Context allUsers = getContext().createContextAsUser(UserHandle.ALL, 0 /* flags */);

            allUsers.registerReceiver(this, new IntentFilter(Intent.ACTION_USER_REMOVED));

            IntentFilter packageFilter = new IntentFilter(Intent.ACTION_PACKAGE_REMOVED);
            packageFilter.addDataScheme("package");
            allUsers.registerReceiver(this, packageFilter);
        }

        @Override
        public void onReceive(Context context, Intent intent) {
            switch (intent.getAction()) {
                case Intent.ACTION_USER_REMOVED:
                    onUserRemoved(intent);
                    break;
                case Intent.ACTION_PACKAGE_REMOVED:
                    onPackageRemoved(intent);
                    break;
                default:
                    Log.e(TAG, "received unexpected intent: " + intent.getAction());
                    break;
            }
        }

        private void onUserRemoved(Intent intent) {
            int userId = intent.getIntExtra(Intent.EXTRA_USER_HANDLE, UserHandle.USER_NULL);
            if (userId != UserHandle.USER_NULL) {
                mHandler.post(() -> notifyUserRemoved(userId));
            }
        }

        private void onPackageRemoved(Intent intent) {
            if (intent.getBooleanExtra(Intent.EXTRA_REPLACING, false)
                    || !intent.getBooleanExtra(Intent.EXTRA_DATA_REMOVED, false)) {
                // Package is being updated rather than uninstalled.
                return;
            }
            int uid = intent.getIntExtra(Intent.EXTRA_UID, -1);
            if (uid != -1) {
                mHandler.post(() -> notifyAppRemoved(uid));
            }
        }
    }

    private final class TetheringService extends IVmTethering.Stub {
        private final TetheringManager tm = getContext().getSystemService(TetheringManager.class);

        @Override
        public void enableVmTethering() {
            final TetheringRequest tr =
                    new TetheringRequest.Builder(TetheringManager.TETHERING_VIRTUAL)
                            .setConnectivityScope(TetheringManager.CONNECTIVITY_SCOPE_GLOBAL)
                            .build();

            StartTetheringCallback startTetheringCallback =
                    new StartTetheringCallback() {
                        @Override
                        public void onTetheringStarted() {
                            Log.i(TAG, "VM tethering started successfully");
                        }

                        @Override
                        public void onTetheringFailed(int resultCode) {
                            Log.e(
                                    TAG,
                                    "VM tethering failed. Result Code: "
                                            + Integer.toString(resultCode));
                        }
                    };
            tm.startTethering(tr, c -> c.run() /* executor */, startTetheringCallback);
        }

        @Override
        public void disableVmTethering() {
            tm.stopTethering(TetheringManager.TETHERING_VIRTUAL);
        }
    }
}
