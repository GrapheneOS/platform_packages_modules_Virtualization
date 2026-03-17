/*
 * Copyright 2026 The Android Open Source Project
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

package com.android.trusty.benchmark;

import static com.google.common.truth.Truth.assertWithMessage;
import static com.google.common.truth.TruthJUnit.assume;

import android.util.Log;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.JUnit4;

import java.io.File;

@RunWith(JUnit4.class)
public class TrustyBenchmarks {
    private static final String TAG = "TrustyBenchmarks";
    private static final String VM_KERNEL_PATH = "/data/local/tmp/trusty_vm/trusty_security_vm.elf";

    @Test
    public void memoryStats() throws Exception {
        File kernelFile = new File(VM_KERNEL_PATH);
        String fileName = kernelFile.getName();
        int dotIndex = fileName.lastIndexOf('.');
        String vmName = fileName.substring(0, dotIndex);
        assertWithMessage(VM_KERNEL_PATH + " file does not exist")
                .that(kernelFile.exists())
                .isTrue();
        assume().withMessage(vmName + " kernel is empty").that(kernelFile.length() > 0).isTrue();
        boolean booted = TrustyJni.bootVm(VM_KERNEL_PATH, vmName);
        assertWithMessage(vmName + " failed to boot").that(booted).isTrue();

        Log.d(TAG, vmName + " started successfully");

        if (!TrustyJni.shutdownVm()) {
            Log.e(TAG, vmName + " Failed to shutdown");
        }
    }
}
