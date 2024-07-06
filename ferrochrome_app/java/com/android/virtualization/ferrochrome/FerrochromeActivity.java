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

import org.apache.commons.compress.archivers.tar.TarArchiveEntry;
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream;
import org.apache.commons.compress.compressors.xz.XZCompressorInputStream;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class FerrochromeActivity extends Activity {
    ExecutorService executorService = Executors.newSingleThreadExecutor();
    private static final String TAG = "FerrochromeActivity";
    private static final String ACTION_VM_LAUNCHER = "android.virtualization.VM_LAUNCHER";
    private static final String FERROCHROME_VERSION = "R128-15926.0.0";
    private static final String EXTERNAL_STORAGE_DIR =
            Environment.getExternalStorageDirectory().getPath() + File.separator;
    private static final Path IMAGE_PATH =
            Path.of(EXTERNAL_STORAGE_DIR + "chromiumos_test_image.bin");
    private static final Path IMAGE_VERSION_INFO =
            Path.of(EXTERNAL_STORAGE_DIR + "ferrochrome_image_version");
    private static final Path VM_CONFIG_PATH = Path.of(EXTERNAL_STORAGE_DIR + "vm_config.json");
    private static final int REQUEST_CODE_VMLAUNCHER = 1;

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
                    if (Files.notExists(IMAGE_PATH)
                            || !FERROCHROME_VERSION.equals(getVersionInfo())) {
                        updateStatus("Starting first-time setup.");
                        updateStatus(
                                "Downloading Ferrochrome image. This can take about 5 to 10"
                                        + " minutes, depending on your network speed.");
                        if (download(FERROCHROME_VERSION)) {
                            updateStatus("Done.");
                        } else {
                            updateStatus(
                                    "Download failed. Check the internet connection and retry.");
                            return;
                        }
                    } else {
                        updateStatus("Ferrochrome is already downloaded.");
                    }
                    updateStatus("Updating VM config.");
                    copyVmConfigJson();
                    updateStatus("Updating VM images. This may take a few minutes.");
                    SystemProperties.set("debug.custom_vm_setup.start", "true");
                    while (!SystemProperties.getBoolean("debug.custom_vm_setup.done", false)) {
                        // Wait for custom_vm_setup
                        try {
                            Thread.sleep(1000);
                        } catch (Exception e) {
                            Log.d(TAG, e.toString());
                        }
                    }
                    updateStatus("Done.");
                    updateStatus("Starting Ferrochrome...");
                    runOnUiThread(() -> startActivityForResult(intent, REQUEST_CODE_VMLAUNCHER));
                });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode == REQUEST_CODE_VMLAUNCHER) {
            finishAndRemoveTask();
        }
    }

    private void updateStatus(String line) {
        Log.d(TAG, line);
        runOnUiThread(
                () -> {
                    TextView statusView = findViewById(R.id.status_txt_view);
                    statusView.append(line + "\n");
                });
    }

    private void copyVmConfigJson() {
        try (InputStream is = getResources().openRawResource(R.raw.vm_config)) {
            Files.copy(is, VM_CONFIG_PATH, StandardCopyOption.REPLACE_EXISTING);
        } catch (IOException e) {
            updateStatus(e.toString());
        }
    }

    private String getVersionInfo() {
        try {
            return new String(Files.readAllBytes(IMAGE_VERSION_INFO), StandardCharsets.UTF_8);
        } catch (IOException e) {
            return null;
        }
    }

    private boolean updateVersionInfo(String version) {
        try {
            Files.write(IMAGE_VERSION_INFO, version.getBytes(StandardCharsets.UTF_8));
        } catch (IOException e) {
            Log.d(TAG, e.toString());
        }
        return true;
    }

    private boolean download(String version) {
        String urlString =
                "https://storage.googleapis.com/chromiumos-image-archive/ferrochrome-public/"
                        + version
                        + "/chromiumos_test_image.tar.xz";
        try (InputStream is = (new URL(urlString)).openStream();
                XZCompressorInputStream xz = new XZCompressorInputStream(is);
                TarArchiveInputStream tar = new TarArchiveInputStream(xz)) {
            TarArchiveEntry entry;
            while ((entry = tar.getNextTarEntry()) != null) {
                if (!entry.getName().contains("chromiumos_test_image.bin")) {
                    continue;
                }
                updateStatus("copy " + entry.getName() + " start");
                Files.copy(tar, IMAGE_PATH, StandardCopyOption.REPLACE_EXISTING);
                updateStatus("copy " + entry.getName() + " done");
                updateVersionInfo(version);
                break;
            }
        } catch (Exception e) {
            updateStatus(e.toString());
            return false;
        }
        return true;
    }
}
