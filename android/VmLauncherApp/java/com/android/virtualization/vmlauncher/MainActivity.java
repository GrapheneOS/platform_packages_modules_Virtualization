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
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineException;
import android.util.Log;
import android.view.SurfaceView;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;

import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
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
    private ClipboardManager mClipboardManager;
    private InputForwarder mInputForwarder;
    private DisplayProvider mDisplayProvider;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        String action = getIntent().getAction();
        if (!ACTION_VM_LAUNCHER.equals(action)) {
            finish();
            Log.e(TAG, "onCreate unsupported intent action: " + action);
            return;
        }
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
        Log.d(TAG, "destroyed");
    }

    private static final int DATA_SHARING_SERVICE_PORT = 3580;
    private static final byte READ_CLIPBOARD_FROM_VM = 0;
    private static final byte WRITE_CLIPBOARD_TYPE_EMPTY = 1;
    private static final byte WRITE_CLIPBOARD_TYPE_TEXT_PLAIN = 2;
    private static final byte OPEN_URL = 3;

    private ClipboardManager getClipboardManager() {
        if (mClipboardManager == null) {
            mClipboardManager = getSystemService(ClipboardManager.class);
        }
        return mClipboardManager;
    }

    // Construct header for the clipboard data.
    // Byte 0: Data type
    // Byte 1-3: Padding alignment & Reserved for other use cases in the future
    // Byte 4-7: Data size of the payload
    private byte[] constructClipboardHeader(byte type, int dataSize) {
        ByteBuffer header = ByteBuffer.allocate(8);
        header.clear();
        header.order(ByteOrder.LITTLE_ENDIAN);
        header.put(0, type);
        header.putInt(4, dataSize);
        return header.array();
    }

    private ParcelFileDescriptor connectDataSharingService() throws VirtualMachineException {
        // TODO(349702313): Consider when clipboard sharing server is started to run in VM.
        return mVirtualMachine.connectVsock(DATA_SHARING_SERVICE_PORT);
    }

    private void writeClipboardToVm() {
        Log.d(TAG, "running writeClipboardToVm");
        try (ParcelFileDescriptor pfd = connectDataSharingService()) {
            ClipboardManager clipboardManager = getClipboardManager();
            if (!clipboardManager.hasPrimaryClip()) {
                Log.d(TAG, "host device has no clipboard data");
                return;
            }
            ClipData clip = clipboardManager.getPrimaryClip();
            String text = clip.getItemAt(0).getText().toString();

            byte[] header =
                    constructClipboardHeader(
                            WRITE_CLIPBOARD_TYPE_TEXT_PLAIN, text.getBytes().length + 1);
            try (OutputStream stream = new FileOutputStream(pfd.getFileDescriptor())) {
                stream.write(header);
                stream.write(text.getBytes());
                stream.write('\0');
                Log.d(TAG, "successfully wrote clipboard data to the VM");
            } catch (IOException e) {
                Log.e(TAG, "failed to write clipboard data to the VM", e);
            }
        } catch (Exception e) {
            Log.e(TAG, "error on writeClipboardToVm", e);
        }
    }

    private byte[] readExactly(InputStream stream, int len) throws IOException {
        byte[] buf = stream.readNBytes(len);
        if (buf.length != len) {
            throw new IOException("Cannot read enough bytes");
        }
        return buf;
    }

    private void readClipboardFromVm() {
        Log.d(TAG, "running readClipboardFromVm");
        try (ParcelFileDescriptor pfd = connectDataSharingService()) {
            byte[] request = constructClipboardHeader(READ_CLIPBOARD_FROM_VM, 0);
            try (OutputStream output = new FileOutputStream(pfd.getFileDescriptor())) {
                output.write(request);
                Log.d(TAG, "successfully send request to the VM for reading clipboard");
            } catch (IOException e) {
                Log.e(TAG, "failed to send request to the VM for reading clipboard");
                throw e;
            }

            try (InputStream input = new FileInputStream(pfd.getFileDescriptor())) {
                ByteBuffer header = ByteBuffer.wrap(readExactly(input, 8));
                header.order(ByteOrder.LITTLE_ENDIAN);
                switch (header.get(0)) {
                    case WRITE_CLIPBOARD_TYPE_EMPTY:
                        Log.d(TAG, "clipboard data in VM is empty");
                        break;
                    case WRITE_CLIPBOARD_TYPE_TEXT_PLAIN:
                        int dataSize = header.getInt(4);
                        String text_data =
                                new String(readExactly(input, dataSize), StandardCharsets.UTF_8);
                        getClipboardManager()
                                .setPrimaryClip(ClipData.newPlainText(null, text_data));
                        Log.d(TAG, "successfully received clipboard data from VM");
                        break;
                    default:
                        Log.e(TAG, "unknown clipboard response type");
                        break;
                }
            } catch (IOException e) {
                Log.e(TAG, "failed to receive clipboard content from VM");
                throw e;
            }
        } catch (Exception e) {
            Log.e(TAG, "error on readClipboardFromVm", e);
        }
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            SurfaceView surfaceView = findViewById(R.id.surface_view);
            Log.d(TAG, "requestPointerCapture()");
            surfaceView.requestPointerCapture();
        }
        if (mVirtualMachine != null) {
            if (hasFocus) {
                mExecutorService.execute(() -> writeClipboardToVm());
            } else {
                mExecutorService.execute(() -> readClipboardFromVm());
            }
        }
    }

    @Override
    protected void onNewIntent(Intent intent) {
        String action = intent.getAction();
        if (!ACTION_VM_OPEN_URL.equals(action)) {
            Log.e(TAG, "onNewIntent unsupported intent action: " + action);
            return;
        }
        Log.d(TAG, "onNewIntent intent action: " + action);
        String text = intent.getStringExtra(Intent.EXTRA_TEXT);
        if (text != null) {
            mExecutorService.execute(
                    () -> {
                        byte[] data = text.getBytes();
                        try (ParcelFileDescriptor pfd = connectDataSharingService();
                                OutputStream stream =
                                        new FileOutputStream(pfd.getFileDescriptor())) {
                            stream.write(constructClipboardHeader(OPEN_URL, data.length));
                            stream.write(data);
                            Log.d(TAG, "Successfully sent URL to the VM");
                        } catch (IOException | VirtualMachineException e) {
                            Log.e(TAG, "Failed to send URL to the VM", e);
                        }
                    });
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
