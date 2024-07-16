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
import android.app.ActivityManager;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.os.Bundle;
import android.os.Environment;
import android.os.SystemProperties;
import android.util.Log;
import android.view.WindowManager;
import android.widget.TextView;

import java.io.BufferedReader;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class FerrochromeActivity extends Activity {
    private static final String TAG = FerrochromeActivity.class.getName();
    private static final String ACTION_VM_LAUNCHER = "android.virtualization.VM_LAUNCHER";

    private static final Path DEST_DIR =
            Path.of(Environment.getExternalStorageDirectory().getPath(), "ferrochrome");
    private static final Path VERSION_FILE = Path.of(DEST_DIR.toString(), "version");

    private static final int REQUEST_CODE_VMLAUNCHER = 1;

    ExecutorService executorService = Executors.newSingleThreadExecutor();

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_ferrochrome);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

        // Find VM Launcher
        Intent intent = new Intent(ACTION_VM_LAUNCHER);
        PackageManager pm = getPackageManager();
        List<ResolveInfo> resolveInfos =
                pm.queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY);
        if (resolveInfos == null || resolveInfos.size() != 1) {
            updateStatus("Failed to resolve VM Launcher");
            return;
        }

        // Clean up the existing vm launcher process if there is
        ActivityManager am = getSystemService(ActivityManager.class);
        am.killBackgroundProcesses(resolveInfos.get(0).activityInfo.packageName);

        executorService.execute(
                () -> {
                    if (updateImageIfNeeded()) {
                        updateStatus("Starting Ferrochrome...");
                        runOnUiThread(
                                () -> startActivityForResult(intent, REQUEST_CODE_VMLAUNCHER));
                    }
                });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode == REQUEST_CODE_VMLAUNCHER) {
            finishAndRemoveTask();
        }
    }

    private boolean updateImageIfNeeded() {
        if (!isUpdateNeeded()) {
            Log.d(TAG, "No update needed.");
            return true;
        }

        try {
            if (Files.notExists(DEST_DIR)) {
                Files.createDirectory(DEST_DIR);
            }

            String[] files = getAssets().list("ferrochrome");
            if (files == null || files.length == 0) {
                updateStatus("ChromeOS image not found. Please go/try-ferrochrome");
                return false;
            }

            updateStatus("Copying images...");
            for (String file : files) {
                updateStatus(file);
                Path dst = Path.of(DEST_DIR.toString(), file);
                updateFile(getAssets().open("ferrochrome/" + file), dst);
            }
        } catch (IOException e) {
            Log.e(TAG, "Error while updating image: " + e);
            updateStatus("Failed.");
            return false;
        }
        updateStatus("Done.");

        updateStatus("Extracting images...");
        SystemProperties.set("debug.custom_vm_setup.start", "true");
        while (!SystemProperties.getBoolean("debug.custom_vm_setup.done", false)) {
            try {
                Thread.sleep(1000);
            } catch (Exception e) {
                Log.e(TAG, "Error while extracting image: " + e);
                updateStatus("Failed.");
                return false;
            }
        }
        // TODO(jiyong): remove this sleep.
        try {
            Thread.sleep(30 * 1000);
        } catch (Exception e) {
            Log.e(TAG, "Interrupted while waiting for the copy to finish");
            updateStatus("Failed.");
            return false;
        }

        updateStatus("Done.");
        return true;
    }

    private boolean isUpdateNeeded() {
        Path[] pathsToCheck = {DEST_DIR, VERSION_FILE};
        for (Path p : pathsToCheck) {
            if (Files.notExists(p)) {
                Log.d(TAG, p.toString() + " does not exist.");
                return true;
            }
        }

        try {
            String installedVer = readLine(new FileInputStream(VERSION_FILE.toFile()));
            String updatedVer = readLine(getAssets().open("ferrochrome/version"));
            if (installedVer.equals(updatedVer)) {
                return false;
            }
            Log.d(TAG, "Version mismatch. Installed: " + installedVer + "  Updated: " + updatedVer);
        } catch (IOException e) {
            Log.e(TAG, "Error while checking version: " + e);
        }
        return true;
    }

    private static String readLine(InputStream input) throws IOException {
        try (BufferedReader reader = new BufferedReader(new InputStreamReader(input))) {
            return reader.readLine();
        } catch (IOException e) {
            throw e;
        }
    }

    private static void updateFile(InputStream input, Path path) throws IOException {
        try {
            Files.copy(input, path, StandardCopyOption.REPLACE_EXISTING);
        } finally {
            input.close();
        }
    }

    private void updateStatus(String line) {
        runOnUiThread(
                () -> {
                    TextView statusView = findViewById(R.id.status_txt_view);
                    statusView.append(line + "\n");
                });
    }
}
