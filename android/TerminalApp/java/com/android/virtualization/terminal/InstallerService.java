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

import static com.android.virtualization.terminal.MainActivity.TAG;

import android.app.Notification;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Build;
import android.os.IBinder;
import android.util.Log;

import androidx.annotation.NonNull;
import androidx.annotation.Nullable;

import com.android.internal.annotations.GuardedBy;

import java.io.IOException;
import java.io.InputStream;
import java.lang.ref.WeakReference;
import java.net.SocketException;
import java.net.UnknownHostException;
import java.nio.file.Path;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class InstallerService extends Service {
    private static final int NOTIFICATION_ID = 1313; // any unique number among notifications

    private final Object mLock = new Object();

    private Notification mNotification;

    @GuardedBy("mLock")
    private boolean mIsInstalling;

    @GuardedBy("mLock")
    private boolean mHasWifi;

    @GuardedBy("mLock")
    private IInstallProgressListener mListener;

    private ExecutorService mExecutorService;
    private ConnectivityManager mConnectivityManager;
    private MyNetworkCallback mNetworkCallback;

    @Override
    public void onCreate() {
        super.onCreate();

        Intent intent = new Intent(this, MainActivity.class);
        PendingIntent pendingIntent =
                PendingIntent.getActivity(
                        this, /* requestCode= */ 0, intent, PendingIntent.FLAG_IMMUTABLE);
        mNotification =
                new Notification.Builder(this, this.getPackageName())
                        .setSilent(true)
                        .setSmallIcon(R.drawable.ic_launcher_foreground)
                        .setContentTitle(getString(R.string.installer_notif_title_text))
                        .setContentText(getString(R.string.installer_notif_desc_text))
                        .setOngoing(true)
                        .setContentIntent(pendingIntent)
                        .build();

        mExecutorService =
                Executors.newSingleThreadExecutor(
                        new TerminalThreadFactory(getApplicationContext()));

        mConnectivityManager = getSystemService(ConnectivityManager.class);
        Network defaultNetwork = mConnectivityManager.getBoundNetworkForProcess();
        if (defaultNetwork != null) {
            NetworkCapabilities capability =
                    mConnectivityManager.getNetworkCapabilities(defaultNetwork);
            if (capability != null) {
                mHasWifi = capability.hasTransport(NetworkCapabilities.TRANSPORT_WIFI);
            }
        }
        mNetworkCallback = new MyNetworkCallback();
        mConnectivityManager.registerDefaultNetworkCallback(mNetworkCallback);
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
        mConnectivityManager.unregisterNetworkCallback(mNetworkCallback);
    }

    private void requestInstall(boolean isWifiOnly) {
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
                    boolean success = downloadFromSdcard() || downloadFromUrl(isWifiOnly);
                    stopForeground(STOP_FOREGROUND_REMOVE);

                    synchronized (mLock) {
                        mIsInstalling = false;
                    }
                    if (success) {
                        notifyCompleted();
                    }
                });
    }

    private boolean downloadFromSdcard() {
        ImageArchive archive = ImageArchive.fromSdCard();

        // Installing from sdcard is preferred, but only supported only in debuggable build.
        if (Build.isDebuggable() && archive.exists()) {
            Log.i(TAG, "trying to install /sdcard/linux/images.tar.gz");

            Path dest = InstalledImage.getDefault(this).getInstallDir();
            try {
                archive.installTo(dest, null);
                Log.i(TAG, "image is installed from /sdcard/linux/images.tar.gz");
                return true;
            } catch (IOException e) {
                Log.i(TAG, "Failed to install /sdcard/linux/images.tar.gz", e);
            }
        } else {
            Log.i(TAG, "Non-debuggable build doesn't support installation from /sdcard/linux");
        }
        return false;
    }

    private boolean checkForWifiOnly(boolean isWifiOnly) {
        if (!isWifiOnly) {
            return true;
        }
        synchronized (mLock) {
            return mHasWifi;
        }
    }

    // TODO(b/374015561): Support pause/resume download
    private boolean downloadFromUrl(boolean isWifiOnly) {
        if (!checkForWifiOnly(isWifiOnly)) {
            Log.e(TAG, "Install isn't started because Wifi isn't available");
            notifyError(getString(R.string.installer_error_no_wifi));
            return false;
        }

        Path dest = InstalledImage.getDefault(this).getInstallDir();
        try {
            ImageArchive.fromInternet()
                    .installTo(
                            dest,
                            is -> {
                                WifiCheckInputStream filter = new WifiCheckInputStream(is);
                                filter.setWifiOnly(isWifiOnly);
                                return filter;
                            });
        } catch (WifiCheckInputStream.NoWifiException e) {
            Log.e(TAG, "Install failed because of Wi-Fi is gone");
            notifyError(getString(R.string.installer_error_no_wifi));
            return false;
        } catch (UnknownHostException | SocketException e) {
            // Log.e() doesn't print stack trace for UnknownHostException
            Log.e(TAG, "Install failed: " + e.getMessage(), e);
            notifyError(getString(R.string.installer_error_network));
            return false;
        } catch (IOException e) {
            Log.e(TAG, "Installation failed", e);
            notifyError(getString(R.string.installer_error_unknown));
            return false;
        }
        return true;
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
        public void requestInstall(boolean isWifiOnly) {
            InstallerService service = ensureServiceConnected();
            synchronized (service.mLock) {
                service.requestInstall(isWifiOnly);
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
                return !service.mIsInstalling && InstalledImage.getDefault(service).isInstalled();
            }
        }
    }

    private final class WifiCheckInputStream extends InputStream {
        private static final int READ_BYTES = 1024;

        private final InputStream mInputStream;
        private boolean mIsWifiOnly;

        public WifiCheckInputStream(InputStream is) {
            super();
            mInputStream = is;
        }

        public void setWifiOnly(boolean isWifiOnly) {
            mIsWifiOnly = isWifiOnly;
        }

        @Override
        public int read(byte[] buf, int offset, int numToRead) throws IOException {
            int totalRead = 0;
            while (numToRead > 0) {
                if (!checkForWifiOnly(mIsWifiOnly)) {
                    throw new NoWifiException();
                }
                int read =
                        mInputStream.read(buf, offset + totalRead, Math.min(READ_BYTES, numToRead));
                if (read <= 0) {
                    break;
                }
                totalRead += read;
                numToRead -= read;
            }
            return totalRead;
        }

        @Override
        public int read() throws IOException {
            if (!checkForWifiOnly(mIsWifiOnly)) {
                throw new NoWifiException();
            }
            return mInputStream.read();
        }

        private static final class NoWifiException extends SocketException {
            // empty
        }
    }

    private final class MyNetworkCallback extends ConnectivityManager.NetworkCallback {
        @Override
        public void onCapabilitiesChanged(
                @NonNull Network network, @NonNull NetworkCapabilities capability) {
            synchronized (mLock) {
                mHasWifi = capability.hasTransport(NetworkCapabilities.TRANSPORT_WIFI);
            }
        }
    }
}
