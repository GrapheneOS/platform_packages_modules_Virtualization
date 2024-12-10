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

import android.os.Build;
import android.os.Environment;
import android.util.Log;

import org.apache.commons.compress.archivers.ArchiveEntry;
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream;
import org.apache.commons.compress.compressors.gzip.GzipCompressorInputStream;

import java.io.BufferedInputStream;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.MalformedURLException;
import java.net.URL;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.Arrays;
import java.util.function.Function;

/**
 * ImageArchive models the archive file (images.tar.gz) where VM payload files are in. This class
 * provides methods for handling the archive file, most importantly installing it.
 */
class ImageArchive {
    private static final String DIR_IN_SDCARD = "linux";
    private static final String ARCHIVE_NAME = "images.tar.gz";
    private static final String BUILD_TAG = Integer.toString(Build.VERSION.SDK_INT_FULL);
    private static final String HOST_URL = "https://dl.google.com/android/ferrochrome/" + BUILD_TAG;

    // Only one can be non-null
    private final URL mUrl;
    private final Path mPath;

    private ImageArchive(URL url) {
        mUrl = url;
        mPath = null;
    }

    private ImageArchive(Path path) {
        mUrl = null;
        mPath = path;
    }

    public static Path getSdcardPathForTesting() {
        return Environment.getExternalStoragePublicDirectory(DIR_IN_SDCARD).toPath();
    }

    /** Creates ImageArchive which is located in the sdcard. This archive is for testing only. */
    public static ImageArchive fromSdCard() {
        Path file = getSdcardPathForTesting().resolve(ARCHIVE_NAME);
        return new ImageArchive(file);
    }

    /** Creates ImageArchive which is hosted in the Google server. This is the official archive. */
    public static ImageArchive fromInternet() {
        String arch = Arrays.asList(Build.SUPPORTED_ABIS).contains("x86_64") ? "x86_64" : "aarch64";
        try {
            URL url = new URL(HOST_URL + "/" + arch + "/" + ARCHIVE_NAME);
            return new ImageArchive(url);
        } catch (MalformedURLException e) {
            // cannot happen
            throw new RuntimeException(e);
        }
    }

    /**
     * Creates ImageArchive from either SdCard or Internet. SdCard is used only when the build is
     * debuggable and the file actually exists.
     */
    public static ImageArchive getDefault() {
        ImageArchive archive = fromSdCard();
        if (Build.isDebuggable() && archive.exists()) {
            return archive;
        } else {
            return fromInternet();
        }
    }

    /** Tests if ImageArchive exists on the medium. */
    public boolean exists() {
        if (mPath != null) {
            return Files.exists(mPath);
        } else {
            // TODO
            return true;
        }
    }

    /** Returns size of the archive in bytes */
    public long getSize() throws IOException {
        if (!exists()) {
            throw new IllegalStateException("Cannot get size of non existing archive");
        }
        if (mPath != null) {
            return Files.size(mPath);
        } else {
            HttpURLConnection conn = null;
            try {
                conn = (HttpURLConnection) mUrl.openConnection();
                conn.setRequestMethod("HEAD");
                conn.getInputStream();
                return conn.getContentLength();
            } finally {
                if (conn != null) {
                    conn.disconnect();
                }
            }
        }
    }

    private InputStream getInputStream(Function<InputStream, InputStream> filter)
            throws IOException {
        InputStream is = mPath != null ? new FileInputStream(mPath.toFile()) : mUrl.openStream();
        BufferedInputStream bufStream = new BufferedInputStream(is);
        return filter == null ? bufStream : filter.apply(bufStream);
    }

    /**
     * Installs this ImageArchive to a directory pointed by path. filter can be supplied to provide
     * an additional input stream which will be used during the installation.
     */
    public void installTo(Path dir, Function<InputStream, InputStream> filter) throws IOException {
        String source = mPath != null ? mPath.toString() : mUrl.toString();
        Log.d(TAG, "Installing. source: " + source + ", destination: " + dir.toString());
        try (InputStream stream = getInputStream(filter);
                GzipCompressorInputStream gzStream = new GzipCompressorInputStream(stream);
                TarArchiveInputStream tarStream = new TarArchiveInputStream(gzStream)) {

            Files.createDirectories(dir);
            ArchiveEntry entry;
            while ((entry = tarStream.getNextEntry()) != null) {
                Path to = dir.resolve(entry.getName());
                if (Files.isDirectory(to)) {
                    Files.createDirectories(to);
                    continue;
                }
                Files.copy(tarStream, to, StandardCopyOption.REPLACE_EXISTING);
            }
        }
        commitInstallationAt(dir);
    }

    private void commitInstallationAt(Path dir) throws IOException {
        // To save storage, delete the source archive on the disk.
        if (mPath != null) {
            Files.deleteIfExists(mPath);
        }

        // Mark the completion
        Path marker = dir.resolve(InstalledImage.MARKER_FILENAME);
        Files.createFile(marker);
    }
}
