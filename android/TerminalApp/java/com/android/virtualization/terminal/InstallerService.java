/*
 * Copyright 2024 The Android Open Source Project
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

package com.android.virtualization.terminal;

import android.app.Notification;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.IBinder;
import android.os.SELinux;
import android.util.Log;

import androidx.annotation.Nullable;

import com.android.internal.annotations.GuardedBy;
import com.android.virtualization.vmlauncher.InstallUtils;

import org.apache.commons.compress.archivers.ArchiveEntry;
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream;
import org.apache.commons.compress.compressors.gzip.GzipCompressorInputStream;

import java.io.BufferedInputStream;
import java.io.File;
import java.io.IOException;
import java.lang.ref.WeakReference;
import java.net.URL;
import java.net.UnknownHostException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.Arrays;
import java.util.Objects;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class InstallerService extends Service {
    private static final String TAG = "InstallerService";

    private static final int NOTIFICATION_ID = 1313; // any unique number among notifications

    private static final String IMAGE_URL =
            Arrays.asList(Build.SUPPORTED_ABIS).contains("x86_64")
                    ? "https://dl.google.com/android/ferrochrome/latest/x86_64/images.tar.gz"
                    : "https://dl.google.com/android/ferrochrome/latest/aarch64/images.tar.gz";

    private static final String SELINUX_FILE_CONTEXT =
            "u:object_r:virtualizationservice_data_file:";

    private final Object mLock = new Object();

    private Notification mNotification;

    @GuardedBy("mLock")
    private boolean mIsInstalling;

    @GuardedBy("mLock")
    private IInstallProgressListener mListener;

    private ExecutorService mExecutorService;

    @Override
    public void onCreate() {
        super.onCreate();

        Intent intent = new Intent(this, MainActivity.class);
        PendingIntent pendingIntent =
                PendingIntent.getActivity(
                        this, /* requestCode= */ 0, intent, PendingIntent.FLAG_IMMUTABLE);
        mNotification =
                new Notification.Builder(this, this.getPackageName())
                        .setSmallIcon(R.drawable.ic_launcher_foreground)
                        .setContentTitle(getString(R.string.installer_notif_title_text))
                        .setContentText(getString(R.string.installer_notif_desc_text))
                        .setOngoing(true)
                        .setContentIntent(pendingIntent)
                        .build();

        mExecutorService = Executors.newSingleThreadExecutor();
    }

    @Nullable
    @Override
    public IBinder onBind(Intent intent) {
        return new InstallerServiceImpl(this);
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        super.onStartCommand(intent, flags, startId);

        Log.d(TAG, "Starting service ...");

        return START_STICKY;
    }

    @Override
    public void onDestroy() {
        super.onDestroy();

        Log.d(TAG, "Service is destroyed");
        if (mExecutorService != null) {
            mExecutorService.shutdown();
        }
    }

    private void requestInstall() {
        synchronized (mLock) {
            if (mIsInstalling) {
                Log.i(TAG, "already installing..");
                return;
            } else {
                Log.i(TAG, "installing..");
                mIsInstalling = true;
            }
        }

        // Make service to be long running, even after unbind() when InstallerActivity is destroyed
        // The service will still be destroyed if task is remove.
        startService(new Intent(this, InstallerService.class));
        startForeground(
                NOTIFICATION_ID, mNotification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);

        mExecutorService.execute(
                () -> {
                    // TODO(b/374015561): Provide progress update
                    boolean success = downloadFromSdcard() || downloadFromUrl();
                    if (success) {
                        reLabelImagesSELinuxContext();
                    }
                    stopForeground(STOP_FOREGROUND_REMOVE);

                    synchronized (mLock) {
                        mIsInstalling = false;
                    }
                    if (success) {
                        notifyCompleted();
                    }
                });
    }

    private void reLabelImagesSELinuxContext() {
        File payloadFolder = InstallUtils.getInternalStorageDir(this);

        // The context should be u:object_r:privapp_data_file:s0:c35,c257,c512,c768
        // and we want to get s0:c35,c257,c512,c768 part
        String level = SELinux.getFileContext(payloadFolder.toString()).split(":", 4)[3];
        String targetContext = SELINUX_FILE_CONTEXT + level;

        File[] files = payloadFolder.listFiles();
        for (File file : files) {
            if (file.isFile() &&
                    !Objects.equals(SELinux.getFileContext(file.toString()),
                            targetContext)) {
                SELinux.setFileContext(file.toString(), targetContext);
            }
        }
    }

    private boolean downloadFromSdcard() {
        // Installing from sdcard is preferred, but only supported only in debuggable build.
        if (Build.isDebuggable()) {
            Log.i(TAG, "trying to install /sdcard/linux/images.tar.gz");

            if (InstallUtils.installImageFromExternalStorage(this)) {
                Log.i(TAG, "image is installed from /sdcard/linux/images.tar.gz");
                return true;
            }
            Log.i(TAG, "Failed to install /sdcard/linux/images.tar.gz");
        } else {
            Log.i(TAG, "Non-debuggable build doesn't support installation from /sdcard/linux");
        }
        return false;
    }

    // TODO(b/374015561): Support pause/resume download
    // TODO(b/374015561): Wait for Wi-Fi on metered network if requested.
    private boolean downloadFromUrl() {
        Log.i(TAG, "trying to download from " + IMAGE_URL);

        try (BufferedInputStream inputStream =
                        new BufferedInputStream(new URL(IMAGE_URL).openStream());
                TarArchiveInputStream tar =
                        new TarArchiveInputStream(new GzipCompressorInputStream(inputStream))) {
            ArchiveEntry entry;
            Path baseDir = InstallUtils.getInternalStorageDir(this).toPath();
            Files.createDirectories(baseDir);
            while ((entry = tar.getNextEntry()) != null) {
                Path extractTo = baseDir.resolve(entry.getName());
                if (entry.isDirectory()) {
                    Files.createDirectories(extractTo);
                } else {
                    Files.copy(tar, extractTo, StandardCopyOption.REPLACE_EXISTING);
                }
            }
        } catch (UnknownHostException e) {
            // Log.e() doesn't print stack trace for UnknownHostException
            Log.e(TAG, "Install failed UnknownHostException: " + e.getMessage());
            notifyError(getString(R.string.installer_install_network_error_message));
            return false;
        } catch (IOException e) {
            // TODO(b/374015561): Provide more finer grained error message
            Log.e(TAG, "Installation failed", e);
            notifyError(getString(R.string.installer_error_unknown));
            return false;
        }

        if (!InstallUtils.resolvePathInVmConfig(this)) {
            // TODO(b/374015561): Provide more finer grained error message
            notifyError(getString(R.string.installer_error_unknown));
            return false;
        }
        return InstallUtils.createInstalledMarker(this);
    }

    private void notifyError(String displayText) {
        IInstallProgressListener listener;
        synchronized (mLock) {
            listener = mListener;
        }

        try {
            listener.onError(displayText);
        } catch (Exception e) {
            // ignore. Activity may not exist.
        }
    }

    private void notifyCompleted() {
        IInstallProgressListener listener;
        synchronized (mLock) {
            listener = mListener;
        }

        try {
            listener.onCompleted();
        } catch (Exception e) {
            // ignore. Activity may not exist.
        }
    }

    private static final class InstallerServiceImpl extends IInstallerService.Stub {
        // Holds weak reference to avoid Context leak
        private final WeakReference<InstallerService> mService;

        public InstallerServiceImpl(InstallerService service) {
            mService = new WeakReference<>(service);
        }

        private InstallerService ensureServiceConnected() throws RuntimeException {
            InstallerService service = mService.get();
            if (service == null) {
                throw new RuntimeException(
                        "Internal error: Installer service is being accessed after destroyed");
            }
            return service;
        }

        @Override
        public void requestInstall() {
            InstallerService service = ensureServiceConnected();
            synchronized (service.mLock) {
                service.requestInstall();
            }
        }

        @Override
        public void setProgressListener(IInstallProgressListener listener) {
            InstallerService service = ensureServiceConnected();
            synchronized (service.mLock) {
                service.mListener = listener;
            }
        }

        @Override
        public boolean isInstalling() {
            InstallerService service = ensureServiceConnected();
            synchronized (service.mLock) {
                return service.mIsInstalling;
            }
        }

        @Override
        public boolean isInstalled() {
            InstallerService service = ensureServiceConnected();
            synchronized (service.mLock) {
                return !service.mIsInstalling && InstallUtils.isImageInstalled(service);
            }
        }
    }
}
