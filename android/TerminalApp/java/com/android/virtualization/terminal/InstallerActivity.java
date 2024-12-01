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

import android.annotation.MainThread;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Build;
import android.os.Bundle;
import android.os.ConditionVariable;
import android.os.FileUtils;
import android.os.IBinder;
import android.os.RemoteException;
import android.text.format.Formatter;
import android.util.Log;
import android.view.KeyEvent;
import android.view.View;
import android.widget.CheckBox;
import android.widget.TextView;

import com.android.internal.annotations.VisibleForTesting;

import com.google.android.material.progressindicator.LinearProgressIndicator;
import com.google.android.material.snackbar.Snackbar;

import java.io.IOException;
import java.lang.ref.WeakReference;

public class InstallerActivity extends BaseActivity {
    private static final long ESTIMATED_IMG_SIZE_BYTES = FileUtils.parseSize("550MB");

    private CheckBox mWaitForWifiCheckbox;
    private TextView mInstallButton;

    private IInstallerService mService;
    private ServiceConnection mInstallerServiceConnection;
    private InstallProgressListener mInstallProgressListener;
    private boolean mInstallRequested;
    private ConditionVariable mInstallCompleted = new ConditionVariable();

    @Override
    public void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setResult(RESULT_CANCELED);

        mInstallProgressListener = new InstallProgressListener(this);

        setContentView(R.layout.activity_installer);
        updateSizeEstimation(ESTIMATED_IMG_SIZE_BYTES);
        measureImageSizeAndUpdateDescription();

        mWaitForWifiCheckbox = (CheckBox) findViewById(R.id.installer_wait_for_wifi_checkbox);
        mInstallButton = (TextView) findViewById(R.id.installer_install_button);

        mInstallButton.setOnClickListener(
                (event) -> {
                    requestInstall();
                });

        Intent intent = new Intent(this, InstallerService.class);
        mInstallerServiceConnection = new InstallerServiceConnection(this);
        if (!bindService(intent, mInstallerServiceConnection, Context.BIND_AUTO_CREATE)) {
            handleInternalError(new Exception("Failed to connect to installer service"));
        }
    }

    private void updateSizeEstimation(long est) {
        String desc =
                getString(
                        R.string.installer_desc_text_format,
                        Formatter.formatShortFileSize(this, est));
        runOnUiThread(
                () -> {
                    TextView view = (TextView) findViewById(R.id.installer_desc);
                    view.setText(desc);
                });
    }

    private void measureImageSizeAndUpdateDescription() {
        new Thread(
                        () -> {
                            long est;
                            try {
                                est = ImageArchive.getDefault().getSize();
                            } catch (IOException e) {
                                Log.w(TAG, "Failed to measure image size.", e);
                                return;
                            }
                            updateSizeEstimation(est);
                        })
                .start();
    }

    @Override
    public void onResume() {
        super.onResume();

        if (Build.isDebuggable() && ImageArchive.fromSdCard().exists()) {
            showSnackbar("Auto installing", Snackbar.LENGTH_LONG);
            requestInstall();
        }
    }

    @Override
    public void onDestroy() {
        if (mInstallerServiceConnection != null) {
            unbindService(mInstallerServiceConnection);
            mInstallerServiceConnection = null;
        }

        super.onDestroy();
    }

    @Override
    public boolean onKeyUp(int keyCode, KeyEvent event) {
        if (keyCode == KeyEvent.KEYCODE_BUTTON_START) {
            requestInstall();
            return true;
        }
        return super.onKeyUp(keyCode, event);
    }

    @VisibleForTesting
    public boolean waitForInstallCompleted(long timeoutMillis) {
        return mInstallCompleted.block(timeoutMillis);
    }

    private void showSnackbar(String message, int length) {
        Snackbar snackbar = Snackbar.make(findViewById(android.R.id.content), message, length);
        snackbar.setAnchorView(mWaitForWifiCheckbox);
        snackbar.show();
    }

    public void handleInternalError(Exception e) {
        if (Build.isDebuggable()) {
            showSnackbar(
                    e.getMessage() + ". File a bugreport to go/ferrochrome-bug",
                    Snackbar.LENGTH_INDEFINITE);
        }
        Log.e(TAG, "Internal error", e);
        finishWithResult(RESULT_CANCELED);
    }

    private void finishWithResult(int resultCode) {
        if (resultCode == RESULT_OK) {
            mInstallCompleted.open();
        }
        setResult(resultCode);
        finish();
    }

    private void setInstallEnabled(boolean enable) {
        mInstallButton.setEnabled(enable);
        mWaitForWifiCheckbox.setEnabled(enable);
        LinearProgressIndicator progressBar = findViewById(R.id.installer_progress);
        if (enable) {
            progressBar.setVisibility(View.INVISIBLE);
        } else {
            progressBar.setVisibility(View.VISIBLE);
        }

        int resId =
                enable
                        ? R.string.installer_install_button_enabled_text
                        : R.string.installer_install_button_disabled_text;
        mInstallButton.setText(getString(resId));
    }

    @MainThread
    private void requestInstall() {
        setInstallEnabled(/* enable= */ false);

        if (mService != null) {
            try {
                mService.requestInstall(mWaitForWifiCheckbox.isChecked());
            } catch (RemoteException e) {
                handleInternalError(e);
            }
        } else {
            Log.d(TAG, "requestInstall() is called, but not yet connected");
            mInstallRequested = true;
        }
    }

    @MainThread
    public void handleInstallerServiceConnected() {
        try {
            mService.setProgressListener(mInstallProgressListener);
            if (mService.isInstalled()) {
                // Finishing this activity will trigger MainActivity::onResume(),
                // and VM will be started from there.
                finishWithResult(RESULT_OK);
                return;
            }

            if (mInstallRequested) {
                requestInstall();
            } else if (mService.isInstalling()) {
                setInstallEnabled(false);
            }
        } catch (RemoteException e) {
            handleInternalError(e);
        }
    }

    @MainThread
    public void handleInstallerServiceDisconnected() {
        handleInternalError(new Exception("InstallerService is destroyed while in use"));
    }

    @MainThread
    private void handleInstallError(String displayText) {
        showSnackbar(displayText, Snackbar.LENGTH_LONG);
        setInstallEnabled(true);
    }

    private static class InstallProgressListener extends IInstallProgressListener.Stub {
        private final WeakReference<InstallerActivity> mActivity;

        InstallProgressListener(InstallerActivity activity) {
            mActivity = new WeakReference<>(activity);
        }

        @Override
        public void onCompleted() {
            InstallerActivity activity = mActivity.get();
            if (activity == null) {
                // Ignore incoming connection or disconnection after activity is destroyed.
                return;
            }

            // MainActivity will be resume and handle rest of progress.
            activity.finishWithResult(RESULT_OK);
        }

        @Override
        public void onError(String displayText) {
            InstallerActivity context = mActivity.get();
            if (context == null) {
                // Ignore incoming connection or disconnection after activity is destroyed.
                return;
            }

            context.runOnUiThread(
                    () -> {
                        InstallerActivity activity = mActivity.get();
                        if (activity == null) {
                            // Ignore incoming connection or disconnection after activity is
                            // destroyed.
                            return;
                        }

                        activity.handleInstallError(displayText);
                    });
        }
    }

    @MainThread
    public static final class InstallerServiceConnection implements ServiceConnection {
        private final WeakReference<InstallerActivity> mActivity;

        InstallerServiceConnection(InstallerActivity activity) {
            mActivity = new WeakReference<>(activity);
        }

        @Override
        public void onServiceConnected(ComponentName name, IBinder service) {
            InstallerActivity activity = mActivity.get();
            if (activity == null || activity.mInstallerServiceConnection == null) {
                // Ignore incoming connection or disconnection after activity is destroyed.
                return;
            }
            if (service == null) {
                activity.handleInternalError(new Exception("service shouldn't be null"));
            }

            activity.mService = IInstallerService.Stub.asInterface(service);
            activity.handleInstallerServiceConnected();
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            InstallerActivity activity = mActivity.get();
            if (activity == null || activity.mInstallerServiceConnection == null) {
                // Ignore incoming connection or disconnection after activity is destroyed.
                return;
            }

            if (activity.mInstallerServiceConnection != null) {
                activity.unbindService(activity.mInstallerServiceConnection);
                activity.mInstallerServiceConnection = null;
            }
            activity.handleInstallerServiceDisconnected();
        }
    }
}
