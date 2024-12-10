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

import static android.webkit.WebSettings.LOAD_NO_CACHE;

import android.app.Notification;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ActivityInfo;
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
import android.os.SystemClock;
import android.provider.Settings;
import android.util.Log;
import android.view.KeyEvent;
import android.view.Menu;
import android.view.MenuItem;
import android.view.View;
import android.view.WindowInsets;
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
import androidx.annotation.NonNull;

import com.android.internal.annotations.VisibleForTesting;

import com.google.android.material.appbar.MaterialToolbar;

import java.io.IOException;
import java.net.InetAddress;
import java.net.MalformedURLException;
import java.net.URL;
import java.net.UnknownHostException;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class MainActivity extends BaseActivity
        implements VmLauncherService.VmLauncherServiceCallback, AccessibilityStateChangeListener {
    static final String TAG = "VmTerminalApp";
    static final String KEY_DISK_SIZE = "disk_size";
    private static final String VM_ADDR = "192.168.0.2";
    private static final int TTYD_PORT = 7681;
    private static final int TERMINAL_CONNECTION_TIMEOUT_MS = 10_000;
    private static final int REQUEST_CODE_INSTALLER = 0x33;
    private static final int FONT_SIZE_DEFAULT = 13;

    private ExecutorService mExecutorService;
    private InstalledImage mImage;
    private X509Certificate[] mCertificates;
    private PrivateKey mPrivateKey;
    private TerminalView mTerminalView;
    private AccessibilityManager mAccessibilityManager;
    private ConditionVariable mBootCompleted = new ConditionVariable();
    private static final int POST_NOTIFICATIONS_PERMISSION_REQUEST_CODE = 101;
    private ActivityResultLauncher<Intent> mManageExternalStorageActivityResultLauncher;
    private static final Map<Integer, Integer> BTN_KEY_CODE_MAP =
            Map.ofEntries(
                    Map.entry(R.id.btn_tab, KeyEvent.KEYCODE_TAB),
                    // Alt key sends ESC keycode
                    Map.entry(R.id.btn_alt, KeyEvent.KEYCODE_ESCAPE),
                    Map.entry(R.id.btn_esc, KeyEvent.KEYCODE_ESCAPE),
                    Map.entry(R.id.btn_left, KeyEvent.KEYCODE_DPAD_LEFT),
                    Map.entry(R.id.btn_right, KeyEvent.KEYCODE_DPAD_RIGHT),
                    Map.entry(R.id.btn_up, KeyEvent.KEYCODE_DPAD_UP),
                    Map.entry(R.id.btn_down, KeyEvent.KEYCODE_DPAD_DOWN),
                    Map.entry(R.id.btn_home, KeyEvent.KEYCODE_MOVE_HOME),
                    Map.entry(R.id.btn_end, KeyEvent.KEYCODE_MOVE_END),
                    Map.entry(R.id.btn_pgup, KeyEvent.KEYCODE_PAGE_UP),
                    Map.entry(R.id.btn_pgdn, KeyEvent.KEYCODE_PAGE_DOWN));

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        lockOrientationIfNecessary();

        mImage = InstalledImage.getDefault(this);

        boolean launchInstaller = installIfNecessary();

        setContentView(R.layout.activity_headless);

        MaterialToolbar toolbar = (MaterialToolbar) findViewById(R.id.toolbar);
        setSupportActionBar(toolbar);
        mTerminalView = (TerminalView) findViewById(R.id.webview);
        mTerminalView.getSettings().setDatabaseEnabled(true);
        mTerminalView.getSettings().setDomStorageEnabled(true);
        mTerminalView.getSettings().setJavaScriptEnabled(true);
        mTerminalView.getSettings().setCacheMode(LOAD_NO_CACHE);
        mTerminalView.setWebChromeClient(new WebChromeClient());

        setupModifierKeys();

        mAccessibilityManager = getSystemService(AccessibilityManager.class);
        mAccessibilityManager.addAccessibilityStateChangeListener(this);

        readClientCertificate();

        mManageExternalStorageActivityResultLauncher =
                registerForActivityResult(
                        new ActivityResultContracts.StartActivityForResult(),
                        (ActivityResult result) -> {
                            startVm();
                        });
        getWindow()
                .getDecorView()
                .getRootView()
                .setOnApplyWindowInsetsListener(
                        (v, insets) -> {
                            updateModifierKeysVisibility();
                            return insets;
                        });

        mExecutorService =
                Executors.newSingleThreadExecutor(
                        new TerminalThreadFactory(getApplicationContext()));

        // if installer is launched, it will be handled in onActivityResult
        if (!launchInstaller) {
            if (!Environment.isExternalStorageManager()) {
                requestStoragePermissions(this, mManageExternalStorageActivityResultLauncher);
            } else {
                startVm();
            }
        }
    }

    private void lockOrientationIfNecessary() {
        boolean hasHwQwertyKeyboard =
                getResources().getConfiguration().keyboard == Configuration.KEYBOARD_QWERTY;
        if (hasHwQwertyKeyboard) {
            setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED);
        } else if (getResources().getBoolean(R.bool.terminal_portrait_only)) {
            setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_PORTRAIT);
        }
    }

    @Override
    public void onConfigurationChanged(@NonNull Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        lockOrientationIfNecessary();
        updateModifierKeysVisibility();
    }

    private void setupModifierKeys() {
        // Only ctrl key is special, it communicates with xtermjs to modify key event with ctrl key
        findViewById(R.id.btn_ctrl)
                .setOnClickListener(
                        (v) -> {
                            mTerminalView.mapCtrlKey();
                            mTerminalView.enableCtrlKey();
                        });

        View.OnClickListener modifierButtonClickListener =
                v -> {
                    if (BTN_KEY_CODE_MAP.containsKey(v.getId())) {
                        int keyCode = BTN_KEY_CODE_MAP.get(v.getId());
                        mTerminalView.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, keyCode));
                        mTerminalView.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, keyCode));
                    }
                };

        for (int btn : BTN_KEY_CODE_MAP.keySet()) {
            View v = findViewById(btn);
            if (v != null) {
                v.setOnClickListener(modifierButtonClickListener);
            }
        }
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        if (Build.isDebuggable() && event.getKeyCode() == KeyEvent.KEYCODE_UNKNOWN) {
            if (event.getAction() == KeyEvent.ACTION_UP) {
                ErrorActivity.start(this, new Exception("Debug: KeyEvent.KEYCODE_UNKNOWN"));
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
        mTerminalView.setWebViewClient(
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
                            case WebViewClient.ERROR_TIMEOUT:
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
                                            findViewById(R.id.webview_container)
                                                    .setVisibility(View.VISIBLE);
                                            mBootCompleted.open();
                                            updateModifierKeysVisibility();
                                            mTerminalView.mapTouchToMouseEvent();
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
        mExecutorService.execute(
                () -> {
                    // TODO(b/376793781): Remove polling
                    waitUntilVmStarts();
                    runOnUiThread(() -> mTerminalView.loadUrl(getTerminalServiceUrl().toString()));
                });
    }

    private static void waitUntilVmStarts() {
        InetAddress addr = null;
        try {
            addr = InetAddress.getByName(VM_ADDR);
        } catch (UnknownHostException e) {
            // this can never happen.
        }

        long startTime = SystemClock.elapsedRealtime();
        while (true) {
            int remainingTime =
                    TERMINAL_CONNECTION_TIMEOUT_MS
                            - (int) (SystemClock.elapsedRealtime() - startTime);
            if (remainingTime <= 0) {
                throw new RuntimeException("Connection to terminal timedout");
            }
            try {
                // Note: this quits immediately if VM is unreachable.
                if (addr.isReachable(remainingTime)) {
                    return;
                }
            } catch (IOException e) {
                // give up on network error
                throw new RuntimeException(e);
            }
        }
    }

    @Override
    protected void onDestroy() {
        if (mExecutorService != null) {
            mExecutorService.shutdown();
        }

        getSystemService(AccessibilityManager.class).removeAccessibilityStateChangeListener(this);
        VmLauncherService.stop(this);
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
        // TODO: error cause is too simple.
        ErrorActivity.start(this, new Exception("onVmError"));
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

    private void updateModifierKeysVisibility() {
        boolean imeShown =
                getWindow().getDecorView().getRootWindowInsets().isVisible(WindowInsets.Type.ime());
        boolean hasHwQwertyKeyboard =
                getResources().getConfiguration().keyboard == Configuration.KEYBOARD_QWERTY;
        boolean showModifierKeys = imeShown && !hasHwQwertyKeyboard;

        View modifierKeys = findViewById(R.id.modifier_keys);
        modifierKeys.setVisibility(showModifierKeys ? View.VISIBLE : View.GONE);
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

    private boolean installIfNecessary() {
        // If payload from external storage exists(only for debuggable build) or there is no
        // installed image, launch installer activity.
        if (!mImage.isInstalled()) {
            Intent intent = new Intent(this, InstallerActivity.class);
            startActivityForResult(intent, REQUEST_CODE_INSTALLER);
            return true;
        }
        return false;
    }

    private void startVm() {
        InstalledImage image = InstalledImage.getDefault(this);
        if (!image.isInstalled()) {
            return;
        }

        resizeDiskIfNecessary(image);

        Intent tapIntent = new Intent(this, MainActivity.class);
        tapIntent.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent tapPendingIntent =
                PendingIntent.getActivity(this, 0, tapIntent, PendingIntent.FLAG_IMMUTABLE);

        Intent settingsIntent = new Intent(this, SettingsActivity.class);
        settingsIntent.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent settingsPendingIntent = PendingIntent.getActivity(this, 0, settingsIntent,
                PendingIntent.FLAG_IMMUTABLE);

        Intent stopIntent = new Intent();
        stopIntent.setClass(this, VmLauncherService.class);
        stopIntent.setAction(VmLauncherService.ACTION_STOP_VM_LAUNCHER_SERVICE);
        PendingIntent stopPendingIntent =
                PendingIntent.getService(
                        this,
                        0,
                        stopIntent,
                        PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        Icon icon = Icon.createWithResource(getResources(), R.drawable.ic_launcher_foreground);
        Notification notification =
                new Notification.Builder(this, this.getPackageName())
                        .setSilent(true)
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
        VmLauncherService.run(this, this, notification);
        connectToTerminalService();
    }

    @VisibleForTesting
    public boolean waitForBootCompleted(long timeoutMillis) {
        return mBootCompleted.block(timeoutMillis);
    }

    private void resizeDiskIfNecessary(InstalledImage image) {
        try {
            // TODO(b/382190982): Show snackbar message instead when it's recoverable.
            image.resize(getIntent().getLongExtra(KEY_DISK_SIZE, image.getSize()));
        } catch (IOException e) {
            ErrorActivity.start(this, new Exception("Failed to resize disk", e));
            return;
        }
    }
}
