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

package com.android.virtualization.linuxinstaller;

import android.annotation.WorkerThread;
import android.app.Activity;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.os.Bundle;
import android.os.Environment;
import android.os.SystemProperties;
import android.text.TextUtils;
import android.util.Log;
import android.widget.TextView;

import libcore.io.Streams;

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

public class MainActivity extends Activity {
    private static final String TAG = "LinuxInstaller";
    private static final String ACTION_VM_TERMINAL = "android.virtualization.VM_TERMINAL";

    private static final Path DEST_DIR =
            Path.of(Environment.getExternalStorageDirectory().getPath(), "linux");

    private static final String ASSET_DIR = "linux";
    private static final String HASH_FILE_NAME = "hash";
    private static final Path HASH_FILE = Path.of(DEST_DIR.toString(), HASH_FILE_NAME);

    ExecutorService executorService = Executors.newSingleThreadExecutor();

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_main);

        executorService.execute(this::installLinuxImage);
    }

    private void installLinuxImage() {
        ComponentName vmTerminalComponent = resolve(getPackageManager(), ACTION_VM_TERMINAL);
        if (vmTerminalComponent == null) {
            updateStatus("Failed to resolve VM terminal");
            return;
        }

        if (!hasLocalAssets()) {
            updateStatus("No local assets");
            return;
        }
        try {
            updateImageIfNeeded();
        } catch (IOException e) {
            Log.e(TAG, "failed to update image", e);
            return;
        }
        updateStatus("Enabling terminal app...");
        getPackageManager()
                .setComponentEnabledSetting(
                        vmTerminalComponent,
                        PackageManager.COMPONENT_ENABLED_STATE_ENABLED,
                        PackageManager.DONT_KILL_APP);
        updateStatus("Done.");
    }

    @WorkerThread
    private boolean hasLocalAssets() {
        try {
            String[] files = getAssets().list(ASSET_DIR);
            return files != null && files.length > 0;
        } catch (IOException e) {
            Log.e(TAG, "there is an error during listing up assets", e);
            return false;
        }
    }

    @WorkerThread
    private void updateImageIfNeeded() throws IOException {
        if (!isUpdateNeeded()) {
            Log.d(TAG, "No update needed.");
            return;
        }

        try {
            if (Files.notExists(DEST_DIR)) {
                Files.createDirectory(DEST_DIR);
            }

            updateStatus("Copying images...");
            String[] files = getAssets().list(ASSET_DIR);
            for (String file : files) {
                updateStatus(file);
                Path dst = Path.of(DEST_DIR.toString(), file);
                updateFile(getAssets().open(ASSET_DIR + "/" + file), dst);
            }
        } catch (IOException e) {
            Log.e(TAG, "Error while updating image: " + e);
            updateStatus("Failed to update image.");
            throw e;
        }
        extractImages(DEST_DIR.toAbsolutePath().toString());
    }

    @WorkerThread
    private void extractImages(String destDir) throws IOException {
        updateStatus("Extracting images...");

        if (TextUtils.isEmpty(destDir)) {
            throw new RuntimeException("Internal error: destDir shouldn't be null");
        }

        SystemProperties.set("debug.custom_vm_setup.path", destDir);
        SystemProperties.set("debug.custom_vm_setup.done", "false");
        SystemProperties.set("debug.custom_vm_setup.start", "true");
        while (!SystemProperties.getBoolean("debug.custom_vm_setup.done", false)) {
            try {
                Thread.sleep(1000);
            } catch (InterruptedException e) {
                Log.e(TAG, "Error while extracting image: " + e);
                updateStatus("Failed to extract image.");
                throw new IOException("extracting image is interrupted", e);
            }
        }
    }

    @WorkerThread
    private boolean isUpdateNeeded() {
        Path[] pathsToCheck = {DEST_DIR, HASH_FILE};
        for (Path p : pathsToCheck) {
            if (Files.notExists(p)) {
                Log.d(TAG, p.toString() + " does not exist.");
                return true;
            }
        }

        try {
            String installedHash = readAll(new FileInputStream(HASH_FILE.toFile()));
            String updatedHash = readAll(getAssets().open(ASSET_DIR + "/" + HASH_FILE_NAME));
            if (installedHash.equals(updatedHash)) {
                return false;
            }
            Log.d(TAG, "Hash mismatch. Installed: " + installedHash + "  Updated: " + updatedHash);
        } catch (IOException e) {
            Log.e(TAG, "Error while checking hash: " + e);
        }
        return true;
    }

    private static String readAll(InputStream input) throws IOException {
        return Streams.readFully(new InputStreamReader(input)).strip();
    }

    private static void updateFile(InputStream input, Path path) throws IOException {
        try (input) {
            Files.copy(input, path, StandardCopyOption.REPLACE_EXISTING);
        }
    }

    private void updateStatus(String line) {
        runOnUiThread(
                () -> {
                    TextView statusView = findViewById(R.id.status_txt_view);
                    statusView.append(line + "\n");
                });
    }

    private ComponentName resolve(PackageManager pm, String action) {
        Intent intent = new Intent(action);
        List<ResolveInfo> resolveInfos = pm.queryIntentActivities(intent, PackageManager.MATCH_ALL);
        if (resolveInfos.size() != 1) {
            Log.w(
                    TAG,
                    "Failed to resolve activity, action=" + action + ", resolved=" + resolveInfos);
            return null;
        }
        ActivityInfo activityInfo = resolveInfos.getFirst().activityInfo;
        // MainActivityAlias shows in Launcher
        return new ComponentName(activityInfo.packageName, activityInfo.name + "Alias");
    }
}
