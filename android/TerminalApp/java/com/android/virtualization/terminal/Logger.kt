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
import java.util.concurrent.ExecutorService
import libcore.io.Streams

/**
 * Forwards VM's console output to a file on the Android side, and VM's log output to Android logd.
 */
internal object Logger {
    fun setup(vm: VirtualMachine, path: Path, executor: ExecutorService) {
        if (vm.config.debugLevel != VirtualMachineConfig.DEBUG_LEVEL_FULL) {
            return
        }

        try {
            val console = vm.getConsoleOutput()
            val file = Files.newOutputStream(path, StandardOpenOption.CREATE)
            executor.submit<Int?> {
                console.use { console ->
                    LineBufferedOutputStream(file).use { fileOutput ->
                        Streams.copy(console, fileOutput)
                    }
                }
            }

            val log = vm.getLogOutput()
            executor.submit<Unit> { log.use { writeToLogd(it, vm.name) } }
        } catch (e: VirtualMachineException) {
            throw RuntimeException(e)
        } catch (e: IOException) {
            throw RuntimeException(e)
        }
    }

    @Throws(IOException::class)
    private fun writeToLogd(input: InputStream?, vmName: String?) {
        val reader = BufferedReader(InputStreamReader(input))
        reader
            .useLines { lines -> lines.takeWhile { !Thread.interrupted() } }
            .forEach { Log.d(vmName, it) }
    }

    private class LineBufferedOutputStream(out: OutputStream?) : BufferedOutputStream(out) {
        @Throws(IOException::class)
        override fun write(buf: ByteArray, off: Int, len: Int) {
            super.write(buf, off, len)
            (0 until len).firstOrNull { buf[off + it] == '\n'.code.toByte() }?.let { flush() }
        }
    }
}
