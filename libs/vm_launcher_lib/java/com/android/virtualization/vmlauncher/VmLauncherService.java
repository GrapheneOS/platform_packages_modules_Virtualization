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

package com.android.virtualization.vmlauncher;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Intent;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.os.ResultReceiver;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineException;
import android.util.Log;

import java.io.BufferedReader;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStreamReader;
import java.nio.file.Path;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class VmLauncherService extends Service {
    private static final String TAG = "VmLauncherService";
    // TODO: this path should be from outside of this service
    private static final String VM_CONFIG_PATH = "/data/local/tmp/vm_config.json";

    private static final int RESULT_START = 0;
    private static final int RESULT_STOP = 1;
    private static final int RESULT_ERROR = 2;
    private static final int RESULT_IPADDR = 3;
    private static final String KEY_VM_IP_ADDR = "ip_addr";

    private ExecutorService mExecutorService;
    private VirtualMachine mVirtualMachine;
    private ResultReceiver mResultReceiver;

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    private void startForeground() {
        NotificationManager notificationManager = getSystemService(NotificationManager.class);
        NotificationChannel notificationChannel =
                new NotificationChannel(TAG, TAG, NotificationManager.IMPORTANCE_LOW);
        notificationManager.createNotificationChannel(notificationChannel);
        startForeground(
                this.hashCode(),
                new Notification.Builder(this, TAG)
                        .setChannelId(TAG)
                        .setSmallIcon(android.R.drawable.ic_dialog_info)
                        .setContentText("A VM " + mVirtualMachine.getName() + " is running")
                        .build());
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (isVmRunning()) {
            Log.d(TAG, "there is already the running VM instance");
            return START_NOT_STICKY;
        }
        mExecutorService = Executors.newCachedThreadPool();

        ConfigJson json = ConfigJson.from(VM_CONFIG_PATH);
        VirtualMachineConfig config = json.toConfig(this);

        Runner runner;
        try {
            runner = Runner.create(this, config);
        } catch (VirtualMachineException e) {
            Log.e(TAG, "cannot create runner", e);
            stopSelf();
            return START_NOT_STICKY;
        }
        mVirtualMachine = runner.getVm();
        mResultReceiver =
                intent.getParcelableExtra(Intent.EXTRA_RESULT_RECEIVER, ResultReceiver.class);

        runner.getExitStatus()
                .thenAcceptAsync(
                        success -> {
                            if (mResultReceiver != null) {
                                mResultReceiver.send(success ? RESULT_STOP : RESULT_ERROR, null);
                            }
                            if (!success) {
                                stopSelf();
                            }
                        });
        Path logPath = getFileStreamPath(mVirtualMachine.getName() + ".log").toPath();
        Logger.setup(mVirtualMachine, logPath, mExecutorService);

        startForeground();

        mResultReceiver.send(RESULT_START, null);
        if (config.getCustomImageConfig().useNetwork()) {
            Handler handler = new Handler(Looper.getMainLooper());
            gatherIpAddrFromVm(handler);
        }
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        super.onDestroy();
        if (isVmRunning()) {
            try {
                mVirtualMachine.stop();
                stopForeground(STOP_FOREGROUND_REMOVE);
            } catch (VirtualMachineException e) {
                Log.e(TAG, "failed to stop a VM instance", e);
            }
            mExecutorService.shutdownNow();
            mExecutorService = null;
            mVirtualMachine = null;
        }
    }

    private boolean isVmRunning() {
        return mVirtualMachine != null
                && mVirtualMachine.getStatus() == VirtualMachine.STATUS_RUNNING;
    }

    // TODO(b/359523803): Use AVF API to get ip addr when it exists
    private void gatherIpAddrFromVm(Handler handler) {
        handler.postDelayed(
                () -> {
                    if (!isVmRunning()) {
                        Log.d(TAG, "A virtual machine instance isn't running");
                        return;
                    }
                    int INTERNAL_VSOCK_SERVER_PORT = 1024;
                    try (ParcelFileDescriptor pfd =
                            mVirtualMachine.connectVsock(INTERNAL_VSOCK_SERVER_PORT)) {
                        try (BufferedReader input =
                                new BufferedReader(
                                        new InputStreamReader(
                                                new FileInputStream(pfd.getFileDescriptor())))) {
                            String vmIpAddr = input.readLine().strip();
                            Bundle b = new Bundle();
                            b.putString(KEY_VM_IP_ADDR, vmIpAddr);
                            mResultReceiver.send(RESULT_IPADDR, b);
                            return;
                        } catch (IOException e) {
                            Log.e(TAG, e.toString());
                        }
                    } catch (Exception e) {
                        Log.e(TAG, e.toString());
                    }
                    gatherIpAddrFromVm(handler);
                },
                1000);
    }
}
