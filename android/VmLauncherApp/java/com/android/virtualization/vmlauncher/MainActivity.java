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

import static android.content.pm.PackageManager.PERMISSION_GRANTED;

import android.Manifest.permission;
import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineException;
import android.util.Log;
import android.view.SurfaceView;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;

import java.nio.file.Path;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class MainActivity extends Activity {
    static final String TAG = "VmLauncherApp";
    // TODO: this path should be from outside of this activity
    private static final String VM_CONFIG_PATH = "/data/local/tmp/vm_config.json";

    private static final int RECORD_AUDIO_PERMISSION_REQUEST_CODE = 101;

    private static final String ACTION_VM_LAUNCHER = "android.virtualization.VM_LAUNCHER";
    private static final String ACTION_VM_OPEN_URL = "android.virtualization.VM_OPEN_URL";

    private ExecutorService mExecutorService;
    private VirtualMachine mVirtualMachine;
    private InputForwarder mInputForwarder;
    private DisplayProvider mDisplayProvider;
    private VmAgent mVmAgent;
    private ClipboardHandler mClipboardHandler;
    private OpenUrlHandler mOpenUrlHandler;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Log.d(TAG, "onCreate intent: " + getIntent());
        checkAndRequestRecordAudioPermission();
        mExecutorService = Executors.newCachedThreadPool();

        ConfigJson json = ConfigJson.from(VM_CONFIG_PATH);
        VirtualMachineConfig config = json.toConfig(this);

        Runner runner;
        try {
            runner = Runner.create(this, config);
        } catch (VirtualMachineException e) {
            throw new RuntimeException(e);
        }
        mVirtualMachine = runner.getVm();
        runner.getExitStatus()
                .thenAcceptAsync(
                        success -> {
                            setResult(success ? RESULT_OK : RESULT_CANCELED);
                            finish();
                        });

        // Setup UI
        setContentView(R.layout.activity_main);
        SurfaceView mainView = findViewById(R.id.surface_view);
        SurfaceView cursorView = findViewById(R.id.cursor_surface_view);
        View touchView = findViewById(R.id.background_touch_view);
        makeFullscreen();

        // Connect the views to the VM
        mInputForwarder = new InputForwarder(this, mVirtualMachine, touchView, mainView, mainView);
        mDisplayProvider = new DisplayProvider(mainView, cursorView);

        Path logPath = getFileStreamPath(mVirtualMachine.getName() + ".log").toPath();
        Logger.setup(mVirtualMachine, logPath, mExecutorService);

        mVmAgent = new VmAgent(mVirtualMachine);
        mClipboardHandler = new ClipboardHandler(this, mVmAgent);
        mOpenUrlHandler = new OpenUrlHandler(mVmAgent);
        handleIntent(getIntent());
    }

    private void makeFullscreen() {
        Window w = getWindow();
        w.setDecorFitsSystemWindows(false);
        WindowInsetsController insetsCtrl = w.getInsetsController();
        insetsCtrl.hide(WindowInsets.Type.systemBars());
        insetsCtrl.setSystemBarsBehavior(
                WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
    }

    @Override
    protected void onResume() {
        super.onResume();
        mInputForwarder.setTabletModeConditionally();
    }

    @Override
    protected void onPause() {
        super.onPause();
        mDisplayProvider.notifyDisplayIsGoingToInvisible();
    }

    @Override
    protected void onStop() {
        super.onStop();
        try {
            mVirtualMachine.suspend();
        } catch (VirtualMachineException e) {
            Log.e(TAG, "Failed to suspend VM" + e);
        }
    }

    @Override
    protected void onRestart() {
        super.onRestart();
        try {
            mVirtualMachine.resume();
        } catch (VirtualMachineException e) {
            Log.e(TAG, "Failed to resume VM" + e);
        }
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        mExecutorService.shutdownNow();
        mInputForwarder.cleanUp();
        mOpenUrlHandler.shutdown();
        Log.d(TAG, "destroyed");
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);

        // TODO: explain why we have to do this on every focus change
        if (hasFocus) {
            SurfaceView mainView = findViewById(R.id.surface_view);
            mainView.requestPointerCapture();
        }

        // TODO: remove executor here. Let clipboard handler handle this.
        mExecutorService.execute(
                () -> {
                    if (hasFocus) {
                        mClipboardHandler.writeClipboardToVm();
                    } else {
                        mClipboardHandler.readClipboardFromVm();
                    }
                });
    }

    @Override
    protected void onNewIntent(Intent intent) {
        Log.d(TAG, "onNewIntent intent: " + intent);
        handleIntent(intent);
    }

    private void handleIntent(Intent intent) {
        if (ACTION_VM_OPEN_URL.equals(intent.getAction())) {
            String url = intent.getStringExtra(Intent.EXTRA_TEXT);
            if (url != null) {
                mOpenUrlHandler.sendUrlToVm(url);
            }
        }
    }

    private void checkAndRequestRecordAudioPermission() {
        if (getApplicationContext().checkSelfPermission(permission.RECORD_AUDIO)
                != PERMISSION_GRANTED) {
            requestPermissions(
                    new String[] {permission.RECORD_AUDIO}, RECORD_AUDIO_PERMISSION_REQUEST_CODE);
        }
    }
}
