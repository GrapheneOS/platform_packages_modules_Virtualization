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

import android.content.Context;
import android.hardware.input.InputManager;
import android.os.Handler;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineCustomImageConfig;
import android.util.Log;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;

/** Forwards input events (touch, mouse, ...) from Android to VM */
class InputForwarder {
    private static final String TAG = MainActivity.TAG;
    private final Context mContext;
    private final VirtualMachine mVirtualMachine;
    private InputManager.InputDeviceListener mInputDeviceListener;

    private boolean isTabletMode = false;

    InputForwarder(
            Context context,
            VirtualMachine vm,
            View touchReceiver,
            View mouseReceiver,
            View keyReceiver) {
        mContext = context;
        mVirtualMachine = vm;

        VirtualMachineCustomImageConfig config = vm.getConfig().getCustomImageConfig();
        if (config.useTouch()) {
            setupTouchReceiver(touchReceiver);
        }
        if (config.useMouse() || config.useTrackpad()) {
            setupMouseReceiver(mouseReceiver);
        }
        if (config.useKeyboard()) {
            setupKeyReceiver(keyReceiver);
        }
        if (config.useSwitches()) {
            // Any view's handler is fine.
            setupTabletModeHandler(touchReceiver.getHandler());
        }
    }

    void cleanUp() {
        if (mInputDeviceListener != null) {
            InputManager im = mContext.getSystemService(InputManager.class);
            im.unregisterInputDeviceListener(mInputDeviceListener);
            mInputDeviceListener = null;
        }
    }

    private void setupTouchReceiver(View receiver) {
        receiver.setOnTouchListener(
                (v, event) -> {
                    return mVirtualMachine.sendMultiTouchEvent(event);
                });
    }

    private void setupMouseReceiver(View receiver) {
        receiver.requestUnbufferedDispatch(InputDevice.SOURCE_ANY);
        receiver.setOnCapturedPointerListener(
                (v, event) -> {
                    int eventSource = event.getSource();
                    if ((eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0) {
                        return mVirtualMachine.sendTrackpadEvent(event);
                    }
                    return mVirtualMachine.sendMouseEvent(event);
                });
    }

    private void setupKeyReceiver(View receiver) {
        receiver.setOnKeyListener(
                (v, code, event) -> {
                    // TODO: this is guest-os specific. It shouldn't be handled here.
                    if (isVolumeKey(code)) {
                        return false;
                    }
                    return mVirtualMachine.sendKeyEvent(event);
                });
    }

    private static boolean isVolumeKey(int keyCode) {
        return keyCode == KeyEvent.KEYCODE_VOLUME_UP
                || keyCode == KeyEvent.KEYCODE_VOLUME_DOWN
                || keyCode == KeyEvent.KEYCODE_VOLUME_MUTE;
    }

    private void setupTabletModeHandler(Handler handler) {
        InputManager im = mContext.getSystemService(InputManager.class);
        mInputDeviceListener =
                new InputManager.InputDeviceListener() {
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
                };
        im.registerInputDeviceListener(mInputDeviceListener, handler);
    }

    private static boolean hasPhysicalKeyboard() {
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice d = InputDevice.getDevice(id);
            if (!d.isVirtual() && d.isEnabled() && d.isFullKeyboard()) {
                return true;
            }
        }
        return false;
    }

    void setTabletModeConditionally() {
        boolean tabletModeNeeded = !hasPhysicalKeyboard();
        if (tabletModeNeeded != isTabletMode) {
            String mode = tabletModeNeeded ? "tablet mode" : "desktop mode";
            Log.d(TAG, "switching to " + mode);
            isTabletMode = tabletModeNeeded;
            mVirtualMachine.sendTabletModeEvent(tabletModeNeeded);
        }
    }
}
