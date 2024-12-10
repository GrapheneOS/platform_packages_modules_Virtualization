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

import static com.android.virtualization.terminal.MainActivity.TAG;

import android.content.Context;
import android.os.FileUtils;
import android.system.ErrnoException;
import android.system.Os;
import android.util.Log;

import java.io.BufferedReader;
import java.io.FileDescriptor;
import java.io.FileReader;
import java.io.IOException;
import java.io.RandomAccessFile;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

/** Collection of files that consist of a VM image. */
class InstalledImage {
    private static final String INSTALL_DIRNAME = "linux";
    private static final String ROOTFS_FILENAME = "root_part";
    private static final String BACKUP_FILENAME = "root_part_backup";
    private static final String CONFIG_FILENAME = "vm_config.json";
    private static final String BUILD_ID_FILENAME = "build_id";
    static final String MARKER_FILENAME = "completed";

    public static final long RESIZE_STEP_BYTES = 4 << 20; // 4 MiB

    private final Path mDir;
    private final Path mRootPartition;
    private final Path mBackup;
    private final Path mConfig;
    private final Path mMarker;
    private String mBuildId;

    /** Returns InstalledImage for a given app context */
    public static InstalledImage getDefault(Context context) {
        Path installDir = context.getFilesDir().toPath().resolve(INSTALL_DIRNAME);
        return new InstalledImage(installDir);
    }

    private InstalledImage(Path dir) {
        mDir = dir;
        mRootPartition = dir.resolve(ROOTFS_FILENAME);
        mBackup = dir.resolve(BACKUP_FILENAME);
        mConfig = dir.resolve(CONFIG_FILENAME);
        mMarker = dir.resolve(MARKER_FILENAME);
    }

    public Path getInstallDir() {
        return mDir;
    }

    /** Tests if this InstalledImage is actually installed. */
    public boolean isInstalled() {
        return Files.exists(mMarker);
    }

    /** Fully understalls this InstalledImage by deleting everything. */
    public void uninstallFully() throws IOException {
        FileUtils.deleteContentsAndDir(mDir.toFile());
    }

    /** Returns the path to the VM config file. */
    public Path getConfigPath() {
        return mConfig;
    }

    /** Returns the build ID of the installed image */
    public String getBuildId() {
        if (mBuildId == null) {
            mBuildId = readBuildId();
        }
        return mBuildId;
    }

    private String readBuildId() {
        Path file = mDir.resolve(BUILD_ID_FILENAME);
        if (!Files.exists(file)) {
            return "<no build id>";
        }
        try (BufferedReader r = new BufferedReader(new FileReader(file.toFile()))) {
            return r.readLine();
        } catch (IOException e) {
            throw new RuntimeException("Failed to read build ID", e);
        }
    }

    public Path uninstallAndBackup() throws IOException {
        Files.delete(mMarker);
        Files.move(mRootPartition, mBackup, StandardCopyOption.REPLACE_EXISTING);
        return mBackup;
    }

    public Path getBackupFile() {
        return mBackup;
    }

    public boolean hasBackup() {
        return Files.exists(mBackup);
    }

    public void deleteBackup() throws IOException {
        Files.deleteIfExists(mBackup);
    }

    public long getSize() throws IOException {
        return Files.size(mRootPartition);
    }

    public long getSmallestSizePossible() throws IOException {
        runE2fsck(mRootPartition);
        String p = mRootPartition.toAbsolutePath().toString();
        String result = runCommand("/system/bin/resize2fs", "-P", p);
        // The return value is the number of 4k block
        try {
            long minSize =
                    Long.parseLong(result.lines().toArray(String[]::new)[1].substring(42))
                            * 4
                            * 1024;
            return roundUp(minSize);
        } catch (NumberFormatException e) {
            Log.e(TAG, "Failed to parse min size, p=" + p + ", result=" + result);
            throw new IOException(e);
        }
    }

    public long resize(long desiredSize) throws IOException {
        desiredSize = roundUp(desiredSize);
        final long curSize = getSize();

        if (desiredSize == curSize) {
            return desiredSize;
        }

        runE2fsck(mRootPartition);
        if (desiredSize > curSize) {
            allocateSpace(mRootPartition, desiredSize);
        }
        resizeFilesystem(mRootPartition, desiredSize);
        return getSize();
    }

    private static void allocateSpace(Path path, long sizeInBytes) throws IOException {
        try {
            RandomAccessFile raf = new RandomAccessFile(path.toFile(), "rw");
            FileDescriptor fd = raf.getFD();
            Os.posix_fallocate(fd, 0, sizeInBytes);
            raf.close();
            Log.d(TAG, "Allocated space to: " + sizeInBytes + " bytes");
        } catch (ErrnoException e) {
            Log.e(TAG, "Failed to allocate space", e);
            throw new IOException("Failed to allocate space", e);
        }
    }

    private static void runE2fsck(Path path) throws IOException {
        String p = path.toAbsolutePath().toString();
        runCommand("/system/bin/e2fsck", "-y", "-f", p);
        Log.d(TAG, "e2fsck completed: " + path);
    }

    private static void resizeFilesystem(Path path, long sizeInBytes) throws IOException {
        long sizeInMB = sizeInBytes / (1024 * 1024);
        if (sizeInMB == 0) {
            Log.e(TAG, "Invalid size: " + sizeInBytes + " bytes");
            throw new IllegalArgumentException("Size cannot be zero MB");
        }
        String sizeArg = sizeInMB + "M";
        String p = path.toAbsolutePath().toString();
        runCommand("/system/bin/resize2fs", p, sizeArg);
        Log.d(TAG, "resize2fs completed: " + path + ", size: " + sizeArg);
    }

    private static String runCommand(String... command) throws IOException {
        try {
            Process process = new ProcessBuilder(command).redirectErrorStream(true).start();
            process.waitFor();
            String result = new String(process.getInputStream().readAllBytes());
            if (process.exitValue() != 0) {
                Log.w(TAG, "Process returned with error, command=" + String.join(" ", command)
                    + ", exitValue=" + process.exitValue() + ", result=" + result);
            }
            return result;
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new IOException("Command interrupted", e);
        }
    }

    private static long roundUp(long bytes) {
        // Round up every diskSizeStep MB
        return (long) Math.ceil(((double) bytes) / RESIZE_STEP_BYTES) * RESIZE_STEP_BYTES;
    }
}
