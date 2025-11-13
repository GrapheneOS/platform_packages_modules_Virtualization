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
package com.android.virtualization.terminal

import android.os.Build
import android.os.Environment
import android.util.Log
import com.android.virtualization.terminal.MainActivity.Companion.TAG
import java.io.BufferedInputStream
import java.io.FileInputStream
import java.io.IOException
import java.io.InputStream
import java.lang.RuntimeException
import java.net.HttpURLConnection
import java.net.MalformedURLException
import java.net.URL
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.util.function.Function
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.launch
import org.apache.commons.compress.archivers.ArchiveEntry
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream
import org.apache.commons.compress.compressors.gzip.GzipCompressorInputStream

/**
 * ImageArchive models the archive file (images.tar.gz) where VM payload files are in. This class
 * provides methods for handling the archive file, most importantly installing it.
 */
internal class ImageArchive {
    // Only one can be non-null
    private sealed class Source<out A, out B>

    private data class UrlSource<out Url>(val value: Url) : Source<Url, Nothing>()

    private data class PathSource<out Path>(val value: Path) : Source<Nothing, Path>()

    private val source: Source<URL, Path>

    private constructor(url: URL) {
        source = UrlSource(url)
    }

    private constructor(path: Path) {
        source = PathSource(path)
    }

    /** Tests if ImageArchive exists on the medium. */
    fun exists(): Boolean {
        return when (source) {
            is UrlSource -> true
            is PathSource -> Files.exists(source.value)
        }
    }

    /** Returns path to the archive. */
    fun getPath(): String {
        return when (source) {
            is UrlSource -> source.value.toString()
            is PathSource -> source.value.toString()
        }
    }

    /** Returns size of the archive in bytes */
    @Throws(IOException::class)
    fun getSize(): Long {
        check(exists()) { "Cannot get size of non existing archive" }
        return when (source) {
            is UrlSource -> {
                val conn = source.value.openConnection() as HttpURLConnection
                try {
                    conn.requestMethod = "HEAD"
                    conn.getInputStream()
                    return conn.contentLength.toLong()
                } finally {
                    conn.disconnect()
                }
            }
            is PathSource -> Files.size(source.value)
        }
    }

    @Throws(IOException::class)
    private fun getInputStream(filter: Function<InputStream, InputStream>?): InputStream? {
        val bufStream =
            BufferedInputStream(
                when (source) {
                    is UrlSource -> source.value.openStream()
                    is PathSource -> FileInputStream(source.value.toFile())
                }
            )
        return filter?.apply(bufStream) ?: bufStream
    }

    /**
     * Installs this ImageArchive to a directory pointed by path. filter can be supplied to provide
     * an additional input stream which will be used during the installation.
     */
    fun installTo(dir: Path, filter: Function<InputStream, InputStream>?): Flow<Long> =
        callbackFlow {
            val job =
                launch(Dispatchers.IO) {
                    val sourceString =
                        when (source) {
                            is PathSource -> source.value.toString()
                            is UrlSource -> source.value.toString()
                        }
                    Log.d(TAG, "Install started. source: $sourceString, destination: $dir")

                    try {
                        val rawStream =
                            when (source) {
                                is UrlSource -> source.value.openStream()
                                is PathSource -> FileInputStream(source.value.toFile())
                            }
                        val bufferedStream = BufferedInputStream(rawStream)
                        val filteredStream = filter?.apply(bufferedStream) ?: bufferedStream
                        val countingStream =
                            CountingInputStream(filteredStream) { bytes -> trySend(bytes) }

                        TarArchiveInputStream(GzipCompressorInputStream(countingStream)).use {
                            tarStream ->
                            Files.createDirectories(dir)
                            var entry: ArchiveEntry?
                            while ((tarStream.nextEntry.also { entry = it }) != null) {
                                val to = dir.resolve(entry!!.getName())
                                if (Files.isDirectory(to)) {
                                    Files.createDirectories(to)
                                    continue
                                }
                                Log.d(TAG, "Installing " + to)
                                Files.copy(tarStream, to, StandardCopyOption.REPLACE_EXISTING)
                            }
                        }
                        Log.d(TAG, "Installed")
                        commitInstallationAt(dir)
                        close()
                    } catch (e: Exception) {
                        close(e)
                    }
                }
            awaitClose { job.cancel() }
        }

    @Throws(IOException::class)
    private fun commitInstallationAt(dir: Path) {
        // Mark the completion
        val marker = dir.resolve(InstalledImage.MARKER_FILENAME)
        Files.createFile(marker)
    }

    private class CountingInputStream(
        private val stream: InputStream,
        private val listener: ((Long) -> Unit)?,
    ) : InputStream() {
        var bytesRead = 0L
            private set

        override fun read(): Int {
            val result = stream.read()
            if (result != -1) {
                bytesRead++
                listener?.invoke(bytesRead)
            }
            return result
        }

        override fun read(b: ByteArray, off: Int, len: Int): Int {
            val result = stream.read(b, off, len)
            if (result != -1) {
                bytesRead += result
                listener?.invoke(bytesRead)
            }
            return result
        }
    }

    companion object {
        private const val DIR_IN_SDCARD = "linux"
        private const val ARCHIVE_NAME = "images.tar.gz"
        // TODO(b/403131508): externalize the version number
        private const val VERSION_INT = 4_000_000 // v4.0.0
        private val BUILD_TAG =
            (VERSION_INT / 1_000_000 * 1_000_000).toString() + // Use only the major version
                "-bug451076949" // TODO(b/451076949): remove this suffix
        private val HOST_URL = "https://dl.google.com/android/ferrochrome/$BUILD_TAG"

        fun getSdcardPathForTesting(): Path {
            return Environment.getExternalStoragePublicDirectory(DIR_IN_SDCARD).toPath()
        }

        /**
         * Creates ImageArchive which is located in the sdcard. This archive is for testing only.
         */
        fun fromSdCard(): ImageArchive {
            return ImageArchive(getSdcardPathForTesting().resolve(ARCHIVE_NAME))
        }

        /**
         * Creates ImageArchive which is hosted in the Google server. This is the official archive.
         */
        fun fromInternet(): ImageArchive {
            val arch =
                if (listOf<String?>(*Build.SUPPORTED_ABIS).contains("x86_64")) "x86_64"
                else "aarch64"
            try {
                return ImageArchive(URL("$HOST_URL/$arch/$ARCHIVE_NAME"))
            } catch (e: MalformedURLException) {
                // cannot happen
                throw RuntimeException(e)
            }
        }

        /**
         * Creates ImageArchive from either SdCard or Internet. SdCard is used only when the build
         * is debuggable and the file actually exists.
         */
        fun getDefault(): ImageArchive {
            val archive = fromSdCard()
            return if (Build.isDebuggable() && archive.exists()) {
                archive
            } else {
                fromInternet()
            }
        }
    }
}
