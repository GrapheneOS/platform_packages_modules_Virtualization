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

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.util.Log;

import java.nio.charset.StandardCharsets;

/** Provide methods to synchronize clipboard across Android and VM. */
class ClipboardHandler {
    private static final String TAG = MainActivity.TAG;
    private final ClipboardManager mClipboardManager;
    private final VmAgent mVmAgent;

    ClipboardHandler(Context context, VmAgent vmAgent) {
        mClipboardManager = context.getSystemService(ClipboardManager.class);
        mVmAgent = vmAgent;
    }

    private VmAgent.Connection getConnection() throws InterruptedException {
        return mVmAgent.connect();
    }

    /** Read a text clip from Android's clipboard and send it to VM. */
    void writeClipboardToVm() {
        if (!mClipboardManager.hasPrimaryClip()) {
            return;
        }

        ClipData clip = mClipboardManager.getPrimaryClip();
        String text = clip.getItemAt(0).getText().toString();
        // TODO: remove this trailing null character. The size is already encoded in the header.
        text = text + '\0';
        // TODO: use UTF-8 encoding
        byte[] data = text.getBytes();

        try {
            getConnection().sendData(VmAgent.WRITE_CLIPBOARD_TYPE_TEXT_PLAIN, data);
        } catch (InterruptedException | RuntimeException e) {
            Log.e(TAG, "Failed to write clipboard data to VM", e);
        }
    }

    /** Read a text clip from VM and paste it to Android's clipboard. */
    void readClipboardFromVm() {
        VmAgent.Data data;
        try {
            data = getConnection().sendAndReceive(VmAgent.READ_CLIPBOARD_FROM_VM, null);
        } catch (InterruptedException | RuntimeException e) {
            Log.e(TAG, "Failed to read clipboard data from VM", e);
            return;
        }

        switch (data.type) {
            case VmAgent.WRITE_CLIPBOARD_TYPE_EMPTY:
                Log.d(TAG, "clipboard data from VM is empty");
                break;
            case VmAgent.WRITE_CLIPBOARD_TYPE_TEXT_PLAIN:
                String text = new String(data.data, StandardCharsets.UTF_8);
                ClipData clip = ClipData.newPlainText(null, text);
                mClipboardManager.setPrimaryClip(clip);
                break;
            default:
                Log.e(TAG, "Unknown clipboard response type: " + data.type);
        }
    }
}
