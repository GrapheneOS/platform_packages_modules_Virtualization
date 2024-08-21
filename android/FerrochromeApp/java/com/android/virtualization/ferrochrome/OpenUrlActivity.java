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

package com.android.virtualization.ferrochrome;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.util.Log;

public class OpenUrlActivity extends Activity {
    private static final String TAG = OpenUrlActivity.class.getSimpleName();

    private static final String ACTION_VM_OPEN_URL = "android.virtualization.VM_OPEN_URL";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        finish();

        if (!Intent.ACTION_SEND.equals(getIntent().getAction())) {
            return;
        }
        String text = getIntent().getStringExtra(Intent.EXTRA_TEXT);
        if (text == null) {
            return;
        }
        Uri uri = Uri.parse(text);
        if (uri == null) {
            return;
        }
        String scheme = uri.getScheme();
        if (!("http".equals(scheme) || "https".equals(scheme) || "mailto".equals(scheme))) {
            Log.e(TAG, "Unsupported URL scheme: " + scheme);
            return;
        }
        Log.i(TAG, "Sending " + scheme + " URL to VM");
        startActivity(
                new Intent(ACTION_VM_OPEN_URL)
                        .setFlags(
                                Intent.FLAG_ACTIVITY_SINGLE_TOP
                                        | Intent.FLAG_ACTIVITY_PREVIOUS_IS_TOP)
                        .putExtra(Intent.EXTRA_TEXT, text));
    }
}
