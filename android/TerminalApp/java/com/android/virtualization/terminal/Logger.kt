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

import android.system.virtualmachine.VirtualMachine
import android.system.virtualmachine.VirtualMachineConfig
import android.system.virtualmachine.VirtualMachineException
import android.util.Log
import com.android.virtualization.terminal.Logger.LineBufferedOutputStream
import java.io.BufferedOutputStream
import java.io.BufferedReader
import java.io.IOException
import java.io.InputStream
import java.io.InputStreamReader
import java.io.OutputStream
import java.lang.RuntimeException
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption
import java.time.LocalDateTime
import java.util.concurrent.ExecutorService
import libcore.io.Streams

/**
 * Forwards VM's console output to a file on the Android side, and VM's log output to Android logd.
 */
internal object Logger {
    fun setup(vm: VirtualMachine, dir: Path, executor: ExecutorService) {
        val tag = vm.name

        if (vm.config.debugLevel != VirtualMachineConfig.DEBUG_LEVEL_FULL) {
            Log.i(tag, "Logs are not captured. Non-debuggable VM.")
            return
        }

        try {
            if (Files.isRegularFile(dir)) {
                Log.i(tag, "Removed legacy log file: $dir")
                Files.delete(dir)
            }
            Files.createDirectories(dir)
            deleteOldLogs(dir, 10)
            val logPath = dir.resolve(LocalDateTime.now().toString() + ".txt")
            val console = vm.getConsoleOutput()
            val file = Files.newOutputStream(logPath, StandardOpenOption.CREATE)
            executor.submit<Int?> {
                console.use { console ->
                    LineBufferedOutputStream(file).use { fileOutput ->
                        Streams.copy(console, fileOutput)
                    }
                }
            }

            val log = vm.getLogOutput()
            executor.submit<Unit> { log.use { writeToLogd(it, tag) } }
        } catch (e: VirtualMachineException) {
            throw RuntimeException(e)
        } catch (e: IOException) {
            throw RuntimeException(e)
        }
    }

    fun deleteOldLogs(dir: Path, numLogsToKeep: Long) {
        Files.list(dir)
            .filter { Files.isRegularFile(it) }
            .sorted(
                Comparator.comparingLong { f: Path ->
                        // for some reason, type inference didn't work here!
                        Files.getLastModifiedTime(f).toMillis()
                    }
                    .reversed()
            )
            .skip(numLogsToKeep)
            .forEach {
                try {
                    Files.delete(it)
                } catch (e: IOException) {
                    // don't bother
                }
            }
    }

    @Throws(IOException::class)
    private fun writeToLogd(input: InputStream?, tag: String?) {
        val reader = BufferedReader(InputStreamReader(input))
        reader
            .useLines { lines -> lines.takeWhile { !Thread.interrupted() } }
            .forEach { Log.d(tag, it) }
    }

    private class LineBufferedOutputStream(out: OutputStream?) : BufferedOutputStream(out) {
        @Throws(IOException::class)
        override fun write(buf: ByteArray, off: Int, len: Int) {
            super.write(buf, off, len)
            (0 until len).firstOrNull { buf[off + it] == '\n'.code.toByte() }?.let { flush() }
        }
    }
}
