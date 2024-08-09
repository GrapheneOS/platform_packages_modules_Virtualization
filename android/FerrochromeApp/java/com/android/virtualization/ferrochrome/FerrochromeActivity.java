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

import android.annotation.WorkerThread;
import android.app.Activity;
import android.app.ActivityManager;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.os.Bundle;
import android.os.Environment;
import android.os.SystemProperties;
import android.text.TextUtils;
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
    private static final String ACTION_FERROCHROME_DOWNLOAD =
            "android.virtualization.FERROCHROME_DOWNLOADER";
    private static final String EXTRA_FERROCHROME_DEST_DIR = "dest_dir";
    private static final String EXTRA_FERROCHROME_UPDATE_NEEDED = "update_needed";

    private static final Path DEST_DIR =
            Path.of(Environment.getExternalStorageDirectory().getPath(), "ferrochrome");
    private static final String ASSET_DIR = "ferrochrome";
    private static final Path VERSION_FILE = Path.of(DEST_DIR.toString(), "version");

    private static final int REQUEST_CODE_VMLAUNCHER = 1;
    private static final int REQUEST_CODE_FERROCHROME_DOWNLOADER = 2;

    private ResolvedActivity mVmLauncher;

    ExecutorService executorService = Executors.newSingleThreadExecutor();

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        if (!isTaskRoot()) {
            // In case we launched this activity multiple times, only start one instance of this
            // activity by only starting this as the root activity in task.
            finish();
            Log.w(TAG, "Not starting because not task root");
            return;
        }
        setContentView(R.layout.activity_ferrochrome);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

        // Find VM Launcher
        mVmLauncher = ResolvedActivity.resolve(getPackageManager(), ACTION_VM_LAUNCHER);
        if (mVmLauncher == null) {
            updateStatus("Failed to resolve VM Launcher");
            return;
        }

        // Clean up the existing vm launcher process if there is
        ActivityManager am = getSystemService(ActivityManager.class);
        am.killBackgroundProcesses(mVmLauncher.activityInfo.packageName);

        executorService.execute(
                () -> {
                    if (hasLocalAssets()) {
                        if (updateImageIfNeeded()) {
                            updateStatus("Starting Ferrochrome...");
                            runOnUiThread(
                                    () ->
                                            startActivityForResult(
                                                    mVmLauncher.intent, REQUEST_CODE_VMLAUNCHER));
                        }
                    } else {
                        tryLaunchDownloader();
                    }
                });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode == REQUEST_CODE_VMLAUNCHER) {
            finishAndRemoveTask();
        } else if (requestCode == REQUEST_CODE_FERROCHROME_DOWNLOADER) {
            String destDir = data.getStringExtra(EXTRA_FERROCHROME_DEST_DIR);
            boolean updateNeeded =
                    data.getBooleanExtra(EXTRA_FERROCHROME_UPDATE_NEEDED, /* default= */ true);

            if (resultCode != RESULT_OK || TextUtils.isEmpty(destDir)) {
                Log.w(
                        TAG,
                        "Ferrochrome downloader returned error, code="
                                + resultCode
                                + ", dest="
                                + destDir);
                updateStatus("User didn't accepted ferrochrome download..");
                return;
            }

            Log.w(TAG, "Ferrochrome downloader returned OK");

            if (!updateNeeded) {
                updateStatus("Starting Ferrochrome...");
                startActivityForResult(mVmLauncher.intent, REQUEST_CODE_VMLAUNCHER);
            }

            executorService.execute(
                    () -> {
                        if (!extractImages(destDir)) {
                            updateStatus("Images from downloader looks bad..");
                            return;
                        }
                        updateStatus("Starting Ferrochrome...");
                        runOnUiThread(
                                () ->
                                        startActivityForResult(
                                                mVmLauncher.intent, REQUEST_CODE_VMLAUNCHER));
                    });
        }
    }

    @WorkerThread
    private boolean hasLocalAssets() {
        try {
            String[] files = getAssets().list(ASSET_DIR);
            return files != null && files.length > 0;
        } catch (IOException e) {
            return false;
        }
    }

    @WorkerThread
    private boolean updateImageIfNeeded() {
        if (!isUpdateNeeded()) {
            Log.d(TAG, "No update needed.");
            return true;
        }

        try {
            if (Files.notExists(DEST_DIR)) {
                Files.createDirectory(DEST_DIR);
            }

            updateStatus("Copying images...");
            String[] files = getAssets().list("ferrochrome");
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

        return extractImages(DEST_DIR.toAbsolutePath().toString());
    }

    @WorkerThread
    private void tryLaunchDownloader() {
        // TODO(jaewan): Add safeguard to check whether ferrochrome downloader is valid.
        Log.w(TAG, "No built-in assets found. Try again with ferrochrome downloader");

        ResolvedActivity downloader =
                ResolvedActivity.resolve(getPackageManager(), ACTION_FERROCHROME_DOWNLOAD);
        if (downloader == null) {
            Log.d(TAG, "Ferrochrome downloader doesn't exist");
            updateStatus("ChromeOS image not found. Please go/try-ferrochrome");
            return;
        }
        String pkgName = downloader.activityInfo.packageName;
        Log.d(TAG, "Resolved Ferrochrome Downloader, pkgName=" + pkgName);
        updateStatus("Launching Ferrochrome downloader for update");

        // onActivityResult() will handle downloader result.
        startActivityForResult(downloader.intent, REQUEST_CODE_FERROCHROME_DOWNLOADER);
    }

    @WorkerThread
    private boolean extractImages(String destDir) {
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
            } catch (Exception e) {
                Log.e(TAG, "Error while extracting image: " + e);
                updateStatus("Failed.");
                return false;
            }
        }

        updateStatus("Done.");
        return true;
    }

    @WorkerThread
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

    private static final class ResolvedActivity {
        public final ActivityInfo activityInfo;
        public final Intent intent;

        private ResolvedActivity(ActivityInfo activityInfo, Intent intent) {
            this.activityInfo = activityInfo;
            this.intent = intent;
        }

        /* synthetic access */
        static ResolvedActivity resolve(PackageManager pm, String action) {
            Intent intent = new Intent(action).setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
            List<ResolveInfo> resolveInfos =
                    pm.queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY);
            if (resolveInfos == null || resolveInfos.size() != 1) {
                Log.w(
                        TAG,
                        "Failed to resolve activity, action="
                                + action
                                + ", resolved="
                                + resolveInfos);
                return null;
            }
            ActivityInfo activityInfo = resolveInfos.get(0).activityInfo;
            intent.setClassName(activityInfo.packageName, activityInfo.name);
            return new ResolvedActivity(activityInfo, intent);
        }
    }
}
