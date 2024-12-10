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

import android.Manifest;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.pm.PackageManager;
import android.os.Bundle;

import androidx.appcompat.app.AppCompatActivity;

public abstract class BaseActivity extends AppCompatActivity {
    private static final int POST_NOTIFICATIONS_PERMISSION_REQUEST_CODE = 101;

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

        if (!(this instanceof ErrorActivity)) {
            Thread currentThread = Thread.currentThread();
            if (!(currentThread.getUncaughtExceptionHandler()
                    instanceof TerminalExceptionHandler)) {
                currentThread.setUncaughtExceptionHandler(
                        new TerminalExceptionHandler(getApplicationContext()));
            }
        }
    }

    @Override
    public void onResume() {
        super.onResume();

        if (getApplicationContext().checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(
                    new String[] {Manifest.permission.POST_NOTIFICATIONS},
                    POST_NOTIFICATIONS_PERMISSION_REQUEST_CODE);
        }
    }
}
