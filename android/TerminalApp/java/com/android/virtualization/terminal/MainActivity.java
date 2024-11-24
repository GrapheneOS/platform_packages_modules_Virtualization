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
package com.android.virtualization.terminal;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.graphics.drawable.Icon;
import android.graphics.fonts.FontStyle;
import android.net.Uri;
import android.net.http.SslError;
import android.os.Build;
import android.os.Bundle;
import android.os.ConditionVariable;
import android.os.Environment;
import android.provider.Settings;
import android.system.ErrnoException;
import android.system.Os;
import android.util.Log;
import android.view.KeyEvent;
import android.view.Menu;
import android.view.MenuItem;
import android.view.View;
import android.view.accessibility.AccessibilityManager;
import android.view.accessibility.AccessibilityManager.AccessibilityStateChangeListener;
import android.webkit.ClientCertRequest;
import android.webkit.SslErrorHandler;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebView;
import android.webkit.WebViewClient;

import androidx.activity.result.ActivityResult;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import com.android.internal.annotations.VisibleForTesting;

import com.google.android.material.appbar.MaterialToolbar;

import java.io.File;
import java.io.FileDescriptor;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.RandomAccessFile;
import java.net.InetAddress;
import java.net.MalformedURLException;
import java.net.URL;
import java.net.UnknownHostException;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;

public class MainActivity extends BaseActivity
        implements VmLauncherServices.VmLauncherServiceCallback, AccessibilityStateChangeListener {
    static final String TAG = "VmTerminalApp";
    private static final String VM_ADDR = "192.168.0.2";
    private static final int TTYD_PORT = 7681;
    private static final int REQUEST_CODE_INSTALLER = 0x33;
    private static final int FONT_SIZE_DEFAULT = 13;

    private X509Certificate[] mCertificates;
    private PrivateKey mPrivateKey;
    private WebView mWebView;
    private AccessibilityManager mAccessibilityManager;
    private ConditionVariable mBootCompleted = new ConditionVariable();
    private static final int POST_NOTIFICATIONS_PERMISSION_REQUEST_CODE = 101;
    private ActivityResultLauncher<Intent> mManageExternalStorageActivityResultLauncher;
    private static int diskSizeStep;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        NotificationManager notificationManager = getSystemService(NotificationManager.class);
        if (notificationManager.getNotificationChannel(this.getPackageName()) == null) {
            NotificationChannel channel =
                    new NotificationChannel(
                            this.getPackageName(),
                            getString(R.string.app_name),
                            NotificationManager.IMPORTANCE_DEFAULT);
            notificationManager.createNotificationChannel(channel);
        }

        boolean launchInstaller = installIfNecessary();

        setContentView(R.layout.activity_headless);
        diskSizeStep = getResources().getInteger(
                R.integer.disk_size_round_up_step_size_in_mb) << 20;

        MaterialToolbar toolbar = (MaterialToolbar) findViewById(R.id.toolbar);
        setSupportActionBar(toolbar);
        mWebView = (WebView) findViewById(R.id.webview);
        mWebView.getSettings().setDatabaseEnabled(true);
        mWebView.getSettings().setDomStorageEnabled(true);
        mWebView.getSettings().setJavaScriptEnabled(true);
        mWebView.setWebChromeClient(new WebChromeClient());

        mAccessibilityManager = getSystemService(AccessibilityManager.class);
        mAccessibilityManager.addAccessibilityStateChangeListener(this);

        readClientCertificate();
        connectToTerminalService();

        mManageExternalStorageActivityResultLauncher =
                registerForActivityResult(
                        new ActivityResultContracts.StartActivityForResult(),
                        (ActivityResult result) -> {
                            startVm();
                        });

        // if installer is launched, it will be handled in onActivityResult
        if (!launchInstaller) {
            if (!Environment.isExternalStorageManager()) {
                requestStoragePermissions(this, mManageExternalStorageActivityResultLauncher);
            } else {
                startVm();
            }
        }
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        if (Build.isDebuggable() && event.getKeyCode() == KeyEvent.KEYCODE_UNKNOWN) {
            if (event.getAction() == KeyEvent.ACTION_UP) {
                launchErrorActivity(new Exception("Debug: KeyEvent.KEYCODE_UNKNOWN"));
            }
            return true;
        }
        return super.dispatchKeyEvent(event);
    }

    private void requestStoragePermissions(
            Context context, ActivityResultLauncher<Intent> activityResultLauncher) {
        Intent intent = new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION);
        Uri uri = Uri.fromParts("package", context.getPackageName(), null);
        intent.setData(uri);
        activityResultLauncher.launch(intent);
    }

    private URL getTerminalServiceUrl() {
        Configuration config = getResources().getConfiguration();

        String query =
                "?fontSize="
                        + (int) (config.fontScale * FONT_SIZE_DEFAULT)
                        + "&fontWeight="
                        + (FontStyle.FONT_WEIGHT_NORMAL + config.fontWeightAdjustment)
                        + "&fontWeightBold="
                        + (FontStyle.FONT_WEIGHT_BOLD + config.fontWeightAdjustment)
                        + "&screenReaderMode="
                        + mAccessibilityManager.isEnabled()
                        + "&titleFixed="
                        + getString(R.string.app_name);

        try {
            return new URL("https", VM_ADDR, TTYD_PORT, "/" + query);
        } catch (MalformedURLException e) {
            // this cannot happen
            return null;
        }
    }

    private void readClientCertificate() {
        KeyStore.PrivateKeyEntry pke = CertificateUtils.createOrGetKey();
        CertificateUtils.writeCertificateToFile(this, pke.getCertificate());
        mPrivateKey = pke.getPrivateKey();
        mCertificates = new X509Certificate[1];
        mCertificates[0] = (X509Certificate) pke.getCertificate();
    }

    private void connectToTerminalService() {
        Log.i(TAG, "URL=" + getTerminalServiceUrl().toString());
        mWebView.setWebViewClient(
                new WebViewClient() {
                    private boolean mLoadFailed = false;
                    private long mRequestId = 0;

                    @Override
                    public boolean shouldOverrideUrlLoading(
                            WebView view, WebResourceRequest request) {
                        return false;
                    }

                    @Override
                    public void onPageStarted(WebView view, String url, Bitmap favicon) {
                        mLoadFailed = false;
                    }

                    @Override
                    public void onReceivedError(
                            WebView view, WebResourceRequest request, WebResourceError error) {
                        mLoadFailed = true;
                        switch (error.getErrorCode()) {
                            case WebViewClient.ERROR_CONNECT:
                            case WebViewClient.ERROR_HOST_LOOKUP:
                            case WebViewClient.ERROR_FAILED_SSL_HANDSHAKE:
                                view.reload();
                                return;
                            default:
                                String url = request.getUrl().toString();
                                CharSequence msg = error.getDescription();
                                Log.e(TAG, "Failed to load " + url + ": " + msg);
                        }
                    }

                    @Override
                    public void onPageFinished(WebView view, String url) {
                        if (mLoadFailed) {
                            return;
                        }

                        mRequestId++;
                        view.postVisualStateCallback(
                                mRequestId,
                                new WebView.VisualStateCallback() {
                                    @Override
                                    public void onComplete(long requestId) {
                                        if (requestId == mRequestId) {
                                            android.os.Trace.endAsyncSection("executeTerminal", 0);
                                            findViewById(R.id.boot_progress)
                                                    .setVisibility(View.GONE);
                                            view.setVisibility(View.VISIBLE);
                                            mBootCompleted.open();
                                        }
                                    }
                                });
                    }

                    @Override
                    public void onReceivedClientCertRequest(
                            WebView view, ClientCertRequest request) {
                        if (mPrivateKey != null && mCertificates != null) {
                            request.proceed(mPrivateKey, mCertificates);
                            return;
                        }
                        super.onReceivedClientCertRequest(view, request);
                    }

                    @Override
                    public void onReceivedSslError(
                            WebView view, SslErrorHandler handler, SslError error) {
                        // ttyd uses self-signed certificate
                        handler.proceed();
                    }
                });
        new Thread(
                        () -> {
                            waitUntilVmStarts();
                            runOnUiThread(
                                    () -> mWebView.loadUrl(getTerminalServiceUrl().toString()));
                        })
                .start();
    }

    private void diskResize(File file, long sizeInBytes) throws IOException {
        try {
            if (sizeInBytes == 0) {
                return;
            }
            String filePath = file.getAbsolutePath();
            Log.d(TAG, "Disk-resize in progress for partition: " + filePath);

            long currentSize = Os.stat(filePath).st_size;
            runE2fsck(filePath);
            if (sizeInBytes > currentSize) {
                allocateSpace(file, sizeInBytes);
            }

            resizeFilesystem(filePath, sizeInBytes);
        } catch (ErrnoException e) {
            Log.e(TAG, "ErrnoException during disk resize", e);
            throw new IOException("ErrnoException during disk resize", e);
        } catch (IOException e) {
            Log.e(TAG, "Failed to resize disk", e);
            throw e;
        }
    }

    private static void allocateSpace(File file, long sizeInBytes) throws IOException {
        try {
            RandomAccessFile raf = new RandomAccessFile(file, "rw");
            FileDescriptor fd = raf.getFD();
            Os.posix_fallocate(fd, 0, sizeInBytes);
            raf.close();
            Log.d(TAG, "Allocated space to: " + sizeInBytes + " bytes");
        } catch (ErrnoException e) {
            Log.e(TAG, "Failed to allocate space", e);
            throw new IOException("Failed to allocate space", e);
        }
    }

    private static void runE2fsck(String filePath) throws IOException {
        try {
            runCommand("/system/bin/e2fsck", "-y", "-f", filePath);
            Log.d(TAG, "e2fsck completed: " + filePath);
        } catch (IOException e) {
            Log.e(TAG, "Failed to run e2fsck", e);
            throw e;
        }
    }

    private static void resizeFilesystem(String filePath, long sizeInBytes) throws IOException {
        long sizeInMB = sizeInBytes / (1024 * 1024);
        if (sizeInMB == 0) {
            Log.e(TAG, "Invalid size: " + sizeInBytes + " bytes");
            throw new IllegalArgumentException("Size cannot be zero MB");
        }
        String sizeArg = sizeInMB + "M";
        try {
            runCommand("/system/bin/resize2fs", filePath, sizeArg);
            Log.d(TAG, "resize2fs completed: " + filePath + ", size: " + sizeArg);
        } catch (IOException e) {
            Log.e(TAG, "Failed to run resize2fs", e);
            throw e;
        }
    }

    private static String runCommand(String... command) throws IOException {
        try {
            Process process = new ProcessBuilder(command).redirectErrorStream(true).start();
            process.waitFor();
            return new String(process.getInputStream().readAllBytes());
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new IOException("Command interrupted", e);
        }
    }

    private static void waitUntilVmStarts() {
        InetAddress addr = null;
        try {
            addr = InetAddress.getByName(VM_ADDR);
        } catch (UnknownHostException e) {
            // this can never happen.
        }
        try {
            while (!addr.isReachable(10000)) {}
        } catch (IOException e) {
            // give up on network error
            throw new RuntimeException(e);
        }
        return;
    }

    @Override
    protected void onDestroy() {
        getSystemService(AccessibilityManager.class).removeAccessibilityStateChangeListener(this);
        VmLauncherServices.stopVmLauncherService(this);
        super.onDestroy();
    }

    @Override
    public void onVmStart() {
        Log.i(TAG, "onVmStart()");
    }

    @Override
    public void onVmStop() {
        Log.i(TAG, "onVmStop()");
        finish();
    }

    @Override
    public void onVmError() {
        Log.i(TAG, "onVmError()");
        launchErrorActivity(new Exception("onVmError"));
    }

    @Override
    public void onIpAddrAvailable(String ipAddr) {
        // TODO: remove this
    }

    @Override
    public boolean onCreateOptionsMenu(Menu menu) {
        getMenuInflater().inflate(R.menu.main_menu, menu);
        return true;
    }

    @Override
    public boolean onOptionsItemSelected(MenuItem item) {
        int id = item.getItemId();
        if (id == R.id.menu_item_settings) {
            Intent intent = new Intent(this, SettingsActivity.class);
            this.startActivity(intent);
            return true;
        }
        return super.onOptionsItemSelected(item);
    }

    @Override
    public void onAccessibilityStateChanged(boolean enabled) {
        connectToTerminalService();
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);

        if (requestCode == REQUEST_CODE_INSTALLER) {
            if (resultCode != RESULT_OK) {
                Log.e(TAG, "Failed to start VM. Installer returned error.");
                finish();
            }
            if (!Environment.isExternalStorageManager()) {
                requestStoragePermissions(this, mManageExternalStorageActivityResultLauncher);
            } else {
                startVm();
            }
        }
    }

    private void launchErrorActivity(Exception e) {
        Intent intent = new Intent(this, ErrorActivity.class);
        intent.putExtra(ErrorActivity.EXTRA_CAUSE, e);
        intent.setFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK | Intent.FLAG_ACTIVITY_NEW_TASK);
        this.startActivity(intent);
    }

    private boolean installIfNecessary() {
        // If payload from external storage exists(only for debuggable build) or there is no
        // installed image, launch installer activity.
        if (!InstallUtils.isImageInstalled(this)) {
            Intent intent = new Intent(this, InstallerActivity.class);
            startActivityForResult(intent, REQUEST_CODE_INSTALLER);
            return true;
        }
        return false;
    }

    private void startVm() {
        if (!InstallUtils.isImageInstalled(this)) {
            return;
        }

        resizeDiskIfNecessary();

        Intent tapIntent = new Intent(this, MainActivity.class);
        tapIntent.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent tapPendingIntent = PendingIntent.getActivity(this, 0, tapIntent,
                PendingIntent.FLAG_IMMUTABLE);

        Intent settingsIntent = new Intent(this, SettingsActivity.class);
        settingsIntent.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent settingsPendingIntent = PendingIntent.getActivity(this, 0, settingsIntent,
                PendingIntent.FLAG_IMMUTABLE);

        Intent stopIntent = new Intent();
        stopIntent.setClass(this, VmLauncherService.class);
        stopIntent.setAction(VmLauncherServices.ACTION_STOP_VM_LAUNCHER_SERVICE);
        PendingIntent stopPendingIntent =
                PendingIntent.getService(
                        this,
                        0,
                        stopIntent,
                        PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        Icon icon = Icon.createWithResource(getResources(), R.drawable.ic_launcher_foreground);
        Notification notification =
                new Notification.Builder(this, this.getPackageName())
                        .setSmallIcon(R.drawable.ic_launcher_foreground)
                        .setContentTitle(
                                getResources().getString(R.string.service_notification_title))
                        .setContentText(
                                getResources().getString(R.string.service_notification_content))
                        .setContentIntent(tapPendingIntent)
                        .setOngoing(true)
                        .addAction(
                                new Notification.Action.Builder(
                                                icon,
                                                getResources()
                                                        .getString(
                                                                R.string
                                                                        .service_notification_settings),
                                        settingsPendingIntent)
                                        .build())
                        .addAction(
                                new Notification.Action.Builder(
                                                icon,
                                                getResources()
                                                        .getString(
                                                                R.string
                                                                        .service_notification_quit_action),
                                                stopPendingIntent)
                                        .build())
                        .build();

        android.os.Trace.beginAsyncSection("executeTerminal", 0);
        VmLauncherServices.startVmLauncherService(this, this, notification);
    }

    @VisibleForTesting
    public boolean waitForBootCompleted(long timeoutMillis) {
        return mBootCompleted.block(timeoutMillis);
    }

    private static long roundUpDiskSize(long diskSize) {
        // Round up every diskSizeStep MB
        return (long) Math.ceil(((double) diskSize) / diskSizeStep) * diskSizeStep;
    }

    public static long getMinFilesystemSize(File file) throws IOException, NumberFormatException {
        try {
            runE2fsck(file.getAbsolutePath());
            String result = runCommand("/system/bin/resize2fs", "-P", file.getAbsolutePath());
            // The return value is the number of 4k block
            long minSize = Long.parseLong(
                    result.lines().toArray(String[]::new)[1].substring(42)) * 4 * 1024;
            return roundUpDiskSize(minSize);
        } catch (IOException | NumberFormatException e) {
            Log.e(TAG, "Failed to get filesystem size", e);
            throw e;
        }
    }

    private static long getFilesystemSize(File file) throws ErrnoException {
        return Os.stat(file.getAbsolutePath()).st_size;
    }

    private void resizeDiskIfNecessary() {
        try {
            File file = InstallUtils.getRootfsFile(this);
            SharedPreferences sharedPref = this.getSharedPreferences(
                    getString(R.string.preference_file_key), Context.MODE_PRIVATE);
            SharedPreferences.Editor editor = sharedPref.edit();

            long currentDiskSize = getFilesystemSize(file);
            long newSizeInBytes = sharedPref.getLong(getString(R.string.preference_disk_size_key),
                    roundUpDiskSize(currentDiskSize));
            editor.putLong(getString(R.string.preference_disk_size_key), newSizeInBytes);
            editor.apply();

            Log.d(TAG, "Current disk size: " + currentDiskSize);
            Log.d(TAG, "Targeting disk size: " + newSizeInBytes);

            if (newSizeInBytes != currentDiskSize) {
                diskResize(file, newSizeInBytes);
            }
        } catch (FileNotFoundException e) {
            Log.d(TAG, "No partition file");
        } catch (IOException | ErrnoException | NumberFormatException e) {
            Log.e(TAG, "Failed to resize disk", e);
        }
    }
}
