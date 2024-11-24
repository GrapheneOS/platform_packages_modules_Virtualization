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
package com.android.virtualization.terminal;

import android.content.Context;
import android.os.Environment;
import android.os.FileUtils;
import android.util.Log;

import com.android.internal.annotations.VisibleForTesting;

import org.apache.commons.compress.archivers.ArchiveEntry;
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream;
import org.apache.commons.compress.compressors.gzip.GzipCompressorInputStream;

import java.io.BufferedInputStream;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.HashMap;
import java.util.Map;
import java.util.function.Function;

public class InstallUtils {
    private static final String TAG = InstallUtils.class.getSimpleName();

    private static final String VM_CONFIG_FILENAME = "vm_config.json";
    private static final String COMPRESSED_PAYLOAD_FILENAME = "images.tar.gz";
    private static final String ROOTFS_FILENAME = "root_part";
    private static final String BACKUP_FILENAME = "root_part_backup";
    private static final String INSTALLATION_COMPLETED_FILENAME = "completed";
    private static final String PAYLOAD_DIR = "linux";

    public static String getVmConfigPath(Context context) {
        return getInternalStorageDir(context).toPath().resolve(VM_CONFIG_FILENAME).toString();
    }

    public static boolean isImageInstalled(Context context) {
        return Files.exists(getInstallationCompletedPath(context));
    }

    public static void backupRootFs(Context context) throws IOException {
        Files.move(
                getRootfsFile(context).toPath(),
                getBackupFile(context).toPath(),
                StandardCopyOption.REPLACE_EXISTING);
    }

    public static boolean createInstalledMarker(Context context) {
        try {
            File file = new File(getInstallationCompletedPath(context).toString());
            return file.createNewFile();
        } catch (IOException e) {
            Log.e(TAG, "Failed to mark install completed", e);
            return false;
        }
    }

    @VisibleForTesting
    public static void deleteInstallation(Context context) {
        FileUtils.deleteContentsAndDir(getInternalStorageDir(context));
    }

    private static Path getPayloadPath() {
        File payloadDir = Environment.getExternalStoragePublicDirectory(PAYLOAD_DIR);
        if (payloadDir == null) {
            Log.d(TAG, "no payload dir: " + payloadDir);
            return null;
        }
        Path payloadPath = payloadDir.toPath().resolve(COMPRESSED_PAYLOAD_FILENAME);
        return payloadPath;
    }

    public static boolean payloadFromExternalStorageExists() {
        return Files.exists(getPayloadPath());
    }

    public static File getInternalStorageDir(Context context) {
        return new File(context.getFilesDir(), PAYLOAD_DIR);
    }

    public static File getBackupFile(Context context) {
        return new File(context.getFilesDir(), BACKUP_FILENAME);
    }

    private static Path getInstallationCompletedPath(Context context) {
        return getInternalStorageDir(context).toPath().resolve(INSTALLATION_COMPLETED_FILENAME);
    }

    public static boolean installImageFromExternalStorage(Context context) {
        if (!payloadFromExternalStorageExists()) {
            Log.d(TAG, "no artifact file from external storage");
            return false;
        }
        Path payloadPath = getPayloadPath();
        try (BufferedInputStream inputStream =
                        new BufferedInputStream(Files.newInputStream(payloadPath));
                TarArchiveInputStream tar =
                        new TarArchiveInputStream(new GzipCompressorInputStream(inputStream))) {
            ArchiveEntry entry;
            Path baseDir = new File(context.getFilesDir(), PAYLOAD_DIR).toPath();
            Files.createDirectories(baseDir);
            while ((entry = tar.getNextEntry()) != null) {
                Path extractTo = baseDir.resolve(entry.getName());
                if (entry.isDirectory()) {
                    Files.createDirectories(extractTo);
                } else {
                    Files.copy(tar, extractTo, StandardCopyOption.REPLACE_EXISTING);
                }
            }
        } catch (IOException e) {
            Log.e(TAG, "installation failed", e);
            return false;
        }
        if (!resolvePathInVmConfig(context)) {
            Log.d(TAG, "resolving path failed");
            try {
                Files.deleteIfExists(Path.of(getVmConfigPath(context)));
            } catch (IOException e) {
                return false;
            }
            return false;
        }

        // remove payload if installation is done.
        try {
            Files.deleteIfExists(payloadPath);
        } catch (IOException e) {
            Log.d(TAG, "failed to remove installed payload", e);
        }

        // Create marker for installation done.
        return createInstalledMarker(context);
    }

    private static Function<String, String> getReplacer(Context context) {
        Map<String, String> rules = new HashMap<>();
        rules.put("\\$PAYLOAD_DIR", new File(context.getFilesDir(), PAYLOAD_DIR).toString());
        rules.put("\\$USER_ID", String.valueOf(context.getUserId()));
        rules.put("\\$PACKAGE_NAME", context.getPackageName());
        String appDataDir = context.getDataDir().toString();
        // TODO: remove this hack
        if (context.getUserId() == 0) {
            appDataDir = "/data/data/" + context.getPackageName();
        }
        rules.put("\\$APP_DATA_DIR", appDataDir);
        return (s) -> {
            for (Map.Entry<String, String> rule : rules.entrySet()) {
                s = s.replaceAll(rule.getKey(), rule.getValue());
            }
            return s;
        };
    }

    public static boolean resolvePathInVmConfig(Context context) {
        try {
            String replacedVmConfig =
                    String.join(
                            "\n",
                            Files.readAllLines(Path.of(getVmConfigPath(context))).stream()
                                    .map(getReplacer(context))
                                    .toList());
            Files.write(Path.of(getVmConfigPath(context)), replacedVmConfig.getBytes());
            return true;
        } catch (IOException e) {
            return false;
        }
    }

    public static File getRootfsFile(Context context) throws FileNotFoundException {
        File file = new File(getInternalStorageDir(context), ROOTFS_FILENAME);
        if (!file.exists()) {
            Log.d(TAG, file.getAbsolutePath() + " - file not found");
            throw new FileNotFoundException("File not found: " + ROOTFS_FILENAME);
        }
        return file;
    }
}
