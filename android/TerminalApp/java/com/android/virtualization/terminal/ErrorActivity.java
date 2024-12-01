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
import android.content.Intent;
import android.os.Bundle;
import android.view.View;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.annotation.Nullable;

public class ErrorActivity extends BaseActivity {
    private static final String EXTRA_CAUSE = "cause";

    public static void start(Context context, Exception e) {
        Intent intent = new Intent(context, ErrorActivity.class);
        intent.putExtra(EXTRA_CAUSE, e);
        intent.setFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK | Intent.FLAG_ACTIVITY_NEW_TASK);
        context.startActivity(intent);
    }

    @Override
    protected void onCreate(@Nullable Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        setContentView(R.layout.activity_error);

        View button = findViewById(R.id.recovery);
        button.setOnClickListener((event) -> launchRecoveryActivity());
    }

    @Override
    protected void onNewIntent(@NonNull Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
    }

    @Override
    public void onResume() {
        super.onResume();

        Intent intent = getIntent();
        Exception e = intent.getParcelableExtra(EXTRA_CAUSE, Exception.class);
        TextView cause = findViewById(R.id.cause);
        if (e != null) {
            cause.setText(getString(R.string.error_code, e.toString()));
        } else {
            cause.setText(null);
        }
    }

    private void launchRecoveryActivity() {
        Intent intent = new Intent(this, SettingsRecoveryActivity.class);
        startActivity(intent);
    }
}
