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

package com.android.virtualization.vmlauncher;

import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineException;
import android.util.Log;

import libcore.io.Streams;

import java.io.BufferedOutputStream;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.concurrent.ExecutorService;

/**
 * Forwards VM's console output to a file on the Android side, and VM's log output to Android logd.
 */
class Logger {
    private Logger() {}

    static void setup(VirtualMachine vm, Path path, ExecutorService executor) {
        if (vm.getConfig().getDebugLevel() != VirtualMachineConfig.DEBUG_LEVEL_FULL) {
            return;
        }

        try {
            InputStream console = vm.getConsoleOutput();
            OutputStream file = Files.newOutputStream(path, StandardOpenOption.CREATE);
            executor.submit(() -> Streams.copy(console, new LineBufferedOutputStream(file)));

            InputStream log = vm.getLogOutput();
            executor.submit(() -> writeToLogd(log, vm.getName()));
        } catch (VirtualMachineException | IOException e) {
            throw new RuntimeException(e);
        }
    }

    private static boolean writeToLogd(InputStream input, String vmName) throws IOException {
        BufferedReader reader = new BufferedReader(new InputStreamReader(input));
        String line;
        while ((line = reader.readLine()) != null && !Thread.interrupted()) {
            Log.d(vmName, line);
        }
        // TODO: find out why javac complains when the return type of this method is void. It
        // (incorrectly?) thinks that IOException should be caught inside the lambda.
        return true;
    }

    private static class LineBufferedOutputStream extends BufferedOutputStream {
        LineBufferedOutputStream(OutputStream out) {
            super(out);
        }

        @Override
        public void write(byte[] buf, int off, int len) throws IOException {
            super.write(buf, off, len);
            for (int i = 0; i < len; ++i) {
                if (buf[off + i] == '\n') {
                    flush();
                    break;
                }
            }
        }
    }
}
