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

import android.content.Context;
import android.util.Log;

public class TerminalExceptionHandler implements Thread.UncaughtExceptionHandler {
    private static final String TAG = "TerminalExceptionHandler";

    private final Context mContext;

    public TerminalExceptionHandler(Context context) {
        mContext = context;
    }

    @Override
    public void uncaughtException(Thread thread, Throwable throwable) {
        Exception exception;
        if (throwable instanceof Exception) {
            exception = (Exception) throwable;
        } else {
            exception = new Exception(throwable);
        }
        try {
            ErrorActivity.start(mContext, exception);
        } catch (Exception ex) {
            Log.wtf(TAG, "Failed to launch error activity for an exception", exception);
        }

        thread.getDefaultUncaughtExceptionHandler().uncaughtException(thread, throwable);
    }
}
