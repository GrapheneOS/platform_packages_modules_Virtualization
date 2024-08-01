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
import android.crosvm.ICrosvmAndroidDisplayService;
import android.graphics.PixelFormat;
import android.hardware.input.InputManager;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.system.virtualizationservice_internal.IVirtualizationServiceInternal;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineCallback;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineException;
import android.system.virtualmachine.VirtualMachineManager;
import android.util.Log;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.SurfaceControl;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;

import libcore.io.IoBridge;

import java.io.BufferedOutputStream;
import java.io.BufferedReader;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class MainActivity extends Activity implements InputManager.InputDeviceListener {
    private static final String TAG = "VmLauncherApp";
    private static final String VM_NAME = "my_custom_vm";

    private static final boolean DEBUG = true;
    private static final int RECORD_AUDIO_PERMISSION_REQUEST_CODE = 101;

    private static final String ACTION_VM_LAUNCHER = "android.virtualization.VM_LAUNCHER";
    private static final String ACTION_VM_OPEN_URL = "android.virtualization.VM_OPEN_URL";

    private ExecutorService mExecutorService;
    private VirtualMachine mVirtualMachine;
    private CursorHandler mCursorHandler;
    private ClipboardManager mClipboardManager;


    private static boolean isVolumeKey(int keyCode) {
        return keyCode == KeyEvent.KEYCODE_VOLUME_UP
                || keyCode == KeyEvent.KEYCODE_VOLUME_DOWN
                || keyCode == KeyEvent.KEYCODE_VOLUME_MUTE;
    }

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        if (mVirtualMachine == null) {
            return false;
        }
        return !isVolumeKey(keyCode) && mVirtualMachine.sendKeyEvent(event);
    }

    @Override
    public boolean onKeyUp(int keyCode, KeyEvent event) {
        if (mVirtualMachine == null) {
            return false;
        }
        return !isVolumeKey(keyCode) && mVirtualMachine.sendKeyEvent(event);
    }

    private void registerInputDeviceListener() {
        InputManager inputManager = getSystemService(InputManager.class);
        if (inputManager == null) {
            Log.e(TAG, "failed to registerInputDeviceListener because InputManager is null");
            return;
        }
        inputManager.registerInputDeviceListener(this, null);
    }

    private void unregisterInputDeviceListener() {
        InputManager inputManager = getSystemService(InputManager.class);
        if (inputManager == null) {
            Log.e(TAG, "failed to unregisterInputDeviceListener because InputManager is null");
            return;
        }
        inputManager.unregisterInputDeviceListener(this);
    }

    private void setTabletModeConditionally() {
        if (mVirtualMachine == null) {
            Log.e(TAG, "failed to setTabletModeConditionally because VirtualMachine is null");
            return;
        }
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice d = InputDevice.getDevice(id);
            if (!d.isVirtual() && d.isEnabled() && d.isFullKeyboard()) {
                Log.d(TAG, "the device has a physical keyboard, turn off tablet mode");
                mVirtualMachine.sendTabletModeEvent(false);
                return;
            }
        }
        mVirtualMachine.sendTabletModeEvent(true);
        Log.d(TAG, "the device doesn't have a physical keyboard, turn on tablet mode");
    }

    @Override
    public void onInputDeviceAdded(int deviceId) {
        setTabletModeConditionally();
    }

    @Override
    public void onInputDeviceRemoved(int deviceId) {
        setTabletModeConditionally();
    }

    @Override
    public void onInputDeviceChanged(int deviceId) {
        setTabletModeConditionally();
    }

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
        try {
            // To ensure that the previous display service is removed.
            IVirtualizationServiceInternal.Stub.asInterface(
                            ServiceManager.waitForService("android.system.virtualizationservice"))
                    .clearDisplayService();
        } catch (RemoteException e) {
            Log.d(TAG, "failed to clearDisplayService");
        }
        getWindow().setDecorFitsSystemWindows(false);
        setContentView(R.layout.activity_main);
        VirtualMachineCallback callback =
                new VirtualMachineCallback() {
                    // store reference to ExecutorService to avoid race condition
                    private final ExecutorService mService = mExecutorService;

                    @Override
                    public void onPayloadStarted(VirtualMachine vm) {
                        // This event is only from Microdroid-based VM. Custom VM shouldn't emit
                        // this.
                    }

                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        // This event is only from Microdroid-based VM. Custom VM shouldn't emit
                        // this.
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        // This event is only from Microdroid-based VM. Custom VM shouldn't emit
                        // this.
                    }

                    @Override
                    public void onError(VirtualMachine vm, int errorCode, String message) {
                        Log.e(TAG, "Error from VM. code: " + errorCode + " (" + message + ")");
                        setResult(RESULT_CANCELED);
                        finish();
                    }

                    @Override
                    public void onStopped(VirtualMachine vm, int reason) {
                        Log.d(TAG, "VM stopped. Reason: " + reason);
                        setResult(RESULT_OK);
                        finish();
                    }
                };

        try {
            VirtualMachineConfig config =
                    VmConfigJson.from("/data/local/tmp/vm_config.json").toConfig(this);
            VirtualMachineManager vmm =
                    getApplication().getSystemService(VirtualMachineManager.class);
            if (vmm == null) {
                Log.e(TAG, "vmm is null");
                return;
            }
            mVirtualMachine = vmm.getOrCreate(VM_NAME, config);
            try {
                mVirtualMachine.setConfig(config);
            } catch (VirtualMachineException e) {
                vmm.delete(VM_NAME);
                mVirtualMachine = vmm.create(VM_NAME, config);
                Log.e(TAG, "error for setting VM config", e);
            }

            Log.d(TAG, "vm start");
            mVirtualMachine.run();
            mVirtualMachine.setCallback(Executors.newSingleThreadExecutor(), callback);
            if (DEBUG) {
                InputStream console = mVirtualMachine.getConsoleOutput();
                InputStream log = mVirtualMachine.getLogOutput();
                OutputStream consoleLogFile =
                        new LineBufferedOutputStream(
                                getApplicationContext().openFileOutput("console.log", 0));
                mExecutorService.execute(new CopyStreamTask("console", console, consoleLogFile));
                mExecutorService.execute(new Reader("log", log));
            }
        } catch (VirtualMachineException | IOException e) {
            throw new RuntimeException(e);
        }

        SurfaceView surfaceView = findViewById(R.id.surface_view);
        SurfaceView cursorSurfaceView = findViewById(R.id.cursor_surface_view);
        cursorSurfaceView.setZOrderMediaOverlay(true);
        View backgroundTouchView = findViewById(R.id.background_touch_view);
        backgroundTouchView.setOnTouchListener(
                (v, event) -> {
                    if (mVirtualMachine == null) {
                        return false;
                    }
                    return mVirtualMachine.sendMultiTouchEvent(event);
                });
        surfaceView.requestUnbufferedDispatch(InputDevice.SOURCE_ANY);
        surfaceView.setOnCapturedPointerListener(
                (v, event) -> {
                    if (mVirtualMachine == null) {
                        return false;
                    }
                    int eventSource = event.getSource();
                    if ((eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0) {
                        return mVirtualMachine.sendTrackpadEvent(event);
                    }
                    return mVirtualMachine.sendMouseEvent(event);
                });
        surfaceView
                .getHolder()
                .addCallback(
                        // TODO(b/331708504): it should be handled in AVF framework.
                        new SurfaceHolder.Callback() {
                            @Override
                            public void surfaceCreated(SurfaceHolder holder) {
                                Log.d(
                                        TAG,
                                        "surface size: "
                                                + holder.getSurfaceFrame().flattenToString());
                                Log.d(
                                        TAG,
                                        "ICrosvmAndroidDisplayService.setSurface("
                                                + holder.getSurface()
                                                + ")");
                                runWithDisplayService(
                                        s ->
                                                s.setSurface(
                                                        holder.getSurface(),
                                                        false /* forCursor */));
                                // TODO  execute the above and the below togther with the same call
                                // to runWithDisplayService. Currently this doesn't work because
                                // setSurface somtimes trigger an exception and as a result
                                // drawSavedFrameForSurface is skipped.
                                runWithDisplayService(
                                        s -> s.drawSavedFrameForSurface(false /* forCursor */));
                            }

                            @Override
                            public void surfaceChanged(
                                    SurfaceHolder holder, int format, int width, int height) {
                                Log.d(
                                        TAG,
                                        "surface changed, width: " + width + ", height: " + height);
                            }

                            @Override
                            public void surfaceDestroyed(SurfaceHolder holder) {
                                Log.d(TAG, "ICrosvmAndroidDisplayService.removeSurface()");
                                runWithDisplayService(
                                        (service) -> service.removeSurface(false /* forCursor */));
                            }
                        });
        cursorSurfaceView.getHolder().setFormat(PixelFormat.RGBA_8888);
        cursorSurfaceView
                .getHolder()
                .addCallback(
                        new SurfaceHolder.Callback() {
                            @Override
                            public void surfaceCreated(SurfaceHolder holder) {
                                try {
                                    ParcelFileDescriptor[] pfds =
                                            ParcelFileDescriptor.createSocketPair();
                                    if (mCursorHandler != null) {
                                        mCursorHandler.interrupt();
                                    }
                                    mCursorHandler =
                                            new CursorHandler(
                                                    surfaceView.getSurfaceControl(),
                                                    cursorSurfaceView.getSurfaceControl(),
                                                    pfds[0]);
                                    mCursorHandler.start();
                                    runWithDisplayService(
                                            (service) -> service.setCursorStream(pfds[1]));
                                } catch (Exception e) {
                                    Log.d(TAG, "failed to run cursor stream handler", e);
                                }
                                Log.d(
                                        TAG,
                                        "ICrosvmAndroidDisplayService.setSurface("
                                                + holder.getSurface()
                                                + ")");
                                runWithDisplayService(
                                        (service) ->
                                                service.setSurface(
                                                        holder.getSurface(), true /* forCursor */));
                            }

                            @Override
                            public void surfaceChanged(
                                    SurfaceHolder holder, int format, int width, int height) {
                                Log.d(
                                        TAG,
                                        "cursor surface changed, width: "
                                                + width
                                                + ", height: "
                                                + height);
                            }

                            @Override
                            public void surfaceDestroyed(SurfaceHolder holder) {
                                Log.d(TAG, "ICrosvmAndroidDisplayService.removeSurface()");
                                runWithDisplayService(
                                        (service) -> service.removeSurface(true /* forCursor */));
                            }
                        });
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

        // Fullscreen:
        WindowInsetsController windowInsetsController = surfaceView.getWindowInsetsController();
        windowInsetsController.setSystemBarsBehavior(
                WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
        windowInsetsController.hide(WindowInsets.Type.systemBars());
        registerInputDeviceListener();
    }

    @Override
    protected void onResume() {
        super.onResume();
        setTabletModeConditionally();
    }

    @Override
    protected void onPause() {
        super.onPause();
        runWithDisplayService(s -> s.saveFrameForSurface(false /* forCursor */));
    }

    @Override
    protected void onStop() {
        super.onStop();
        if (mVirtualMachine != null) {
            try {
                mVirtualMachine.sendLidEvent(/* close */ true);
                mVirtualMachine.suspend();
            } catch (VirtualMachineException e) {
                Log.e(TAG, "Failed to suspend VM" + e);
            }
        }
    }

    @Override
    protected void onRestart() {
        super.onRestart();
        if (mVirtualMachine != null) {
            try {
                mVirtualMachine.resume();
                mVirtualMachine.sendLidEvent(/* close */ false);
            } catch (VirtualMachineException e) {
                Log.e(TAG, "Failed to resume VM" + e);
            }
        }
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        if (mExecutorService != null) {
            mExecutorService.shutdownNow();
        }
        unregisterInputDeviceListener();
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

    @FunctionalInterface
    public interface RemoteExceptionCheckedFunction<T> {
        void apply(T t) throws RemoteException;
    }

    private void runWithDisplayService(
            RemoteExceptionCheckedFunction<ICrosvmAndroidDisplayService> func) {
        IVirtualizationServiceInternal vs =
                IVirtualizationServiceInternal.Stub.asInterface(
                        ServiceManager.waitForService("android.system.virtualizationservice"));
        try {
            Log.d(TAG, "wait for the display service");
            ICrosvmAndroidDisplayService service =
                    ICrosvmAndroidDisplayService.Stub.asInterface(vs.waitDisplayService());
            assert service != null;
            func.apply(service);
            Log.d(TAG, "display service runs successfully");
        } catch (Exception e) {
            Log.d(TAG, "error on running display service", e);
        }
    }

    static class CursorHandler extends Thread {
        private final SurfaceControl mCursor;
        private final ParcelFileDescriptor mStream;
        private final SurfaceControl.Transaction mTransaction;

        CursorHandler(SurfaceControl main, SurfaceControl cursor, ParcelFileDescriptor stream) {
            mCursor = cursor;
            mStream = stream;
            mTransaction = new SurfaceControl.Transaction();

            mTransaction.reparent(cursor, main).apply();
        }

        @Override
        public void run() {
            Log.d(TAG, "running CursorHandler");
            try {
                ByteBuffer byteBuffer = ByteBuffer.allocate(8 /* (x: u32, y: u32) */);
                byteBuffer.order(ByteOrder.LITTLE_ENDIAN);
                while (true) {
                    if (Thread.interrupted()) {
                        Log.d(TAG, "interrupted: exiting CursorHandler");
                        return;
                    }
                    byteBuffer.clear();
                    int bytes =
                            IoBridge.read(
                                    mStream.getFileDescriptor(),
                                    byteBuffer.array(),
                                    0,
                                    byteBuffer.array().length);
                    if (bytes == -1) {
                        Log.e(TAG, "cannot read from cursor stream, stop the handler");
                        return;
                    }
                    float x = (float) (byteBuffer.getInt() & 0xFFFFFFFF);
                    float y = (float) (byteBuffer.getInt() & 0xFFFFFFFF);
                    mTransaction.setPosition(mCursor, x, y).apply();
                }
            } catch (IOException e) {
                Log.e(TAG, "failed to run CursorHandler", e);
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

    /** Reads data from an input stream and posts it to the output data */
    static class Reader implements Runnable {
        private final String mName;
        private final InputStream mStream;

        Reader(String name, InputStream stream) {
            mName = name;
            mStream = stream;
        }

        @Override
        public void run() {
            try {
                BufferedReader reader = new BufferedReader(new InputStreamReader(mStream));
                String line;
                while ((line = reader.readLine()) != null && !Thread.interrupted()) {
                    Log.d(TAG, mName + ": " + line);
                }
            } catch (IOException e) {
                Log.e(TAG, "Exception while posting " + mName + " output: " + e.getMessage());
            }
        }
    }

    private static class CopyStreamTask implements Runnable {
        private final String mName;
        private final InputStream mIn;
        private final OutputStream mOut;

        CopyStreamTask(String name, InputStream in, OutputStream out) {
            mName = name;
            mIn = in;
            mOut = out;
        }

        @Override
        public void run() {
            try {
                byte[] buffer = new byte[2048];
                while (!Thread.interrupted()) {
                    int len = mIn.read(buffer);
                    if (len < 0) {
                        break;
                    }
                    mOut.write(buffer, 0, len);
                }
            } catch (Exception e) {
                Log.e(TAG, "Exception while posting " + mName, e);
            }
        }
    }

    private static class LineBufferedOutputStream extends BufferedOutputStream {
        LineBufferedOutputStream(OutputStream out) {
            super(out);
        }

        @Override
        public void write(byte[] buf, int off, int len) throws IOException {
            super.write(buf, off, len);
            for (int i = 0; i < len; ++i) {
                if (buf[off + i] == '\n') {
                    flush();
                    break;
                }
            }
        }
    }
}
