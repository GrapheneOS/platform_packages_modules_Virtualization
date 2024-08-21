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

import android.util.Log;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

class OpenUrlHandler {
    private static final String TAG = MainActivity.TAG;

    private final VmAgent mVmAgent;
    private final ExecutorService mExecutorService;

    OpenUrlHandler(VmAgent vmAgent) {
        mVmAgent = vmAgent;
        mExecutorService = Executors.newSingleThreadExecutor();
    }

    void shutdown() {
        mExecutorService.shutdownNow();
    }

    void sendUrlToVm(String url) {
        mExecutorService.execute(
                () -> {
                    try {
                        mVmAgent.connect().sendData(VmAgent.OPEN_URL, url.getBytes());
                        Log.d(TAG, "Successfully sent URL to the VM");
                    } catch (InterruptedException | RuntimeException e) {
                        Log.e(TAG, "Failed to send URL to the VM", e);
                    }
                });
    }
}
