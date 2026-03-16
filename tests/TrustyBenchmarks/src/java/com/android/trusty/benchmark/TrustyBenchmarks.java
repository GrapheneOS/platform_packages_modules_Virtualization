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

import static androidx.test.platform.app.InstrumentationRegistry.getInstrumentation;

import static com.google.common.truth.Truth.assertWithMessage;
import static com.google.common.truth.TruthJUnit.assume;

import android.os.Bundle;
import android.util.Log;

import com.android.compatibility.common.util.SystemUtil;
import com.android.microdroid.test.common.ProcessUtil;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.JUnit4;

import java.io.File;
import java.util.Map;

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
        assume().withMessage(vmName + " kernel is empty")
                .that(kernelFile.length())
                .isGreaterThan(0L);
        boolean booted = TrustyJni.bootVm(VM_KERNEL_PATH, vmName);
        assertWithMessage(vmName + " failed to boot").that(booted).isTrue();

        Log.d(TAG, vmName + " started successfully");
        double[] stats;
        stats = fetchMemoryStats(vmName);

        Bundle status = new Bundle();
        status.putDouble(vmName + "/memory/rss_mb", stats[0]);
        status.putDouble(vmName + "/memory/dirty_mb", stats[1]);
        getInstrumentation().sendStatus(0, status);
    }

    private double[] fetchMemoryStats(String vmName) throws Exception {
        String pidStr = SystemUtil.runShellCommand("pidof -s crosvm_" + vmName).trim();
        assertWithMessage("crosvm_" + vmName + " process not found").that(pidStr).isNotEmpty();

        int pid = Integer.parseInt(pidStr);

        Map<String, Long> smaps =
                ProcessUtil.getProcessSmapsRollup(pid, SystemUtil::runShellCommand);

        long rssKb = smaps.getOrDefault("Rss", 0L);
        long sharedDirtyKb = smaps.getOrDefault("Shared_Dirty", 0L);
        long privateDirtyKb = smaps.getOrDefault("Private_Dirty", 0L);

        double rssMb = rssKb / 1024.0;
        double dirtyMb = (sharedDirtyKb + privateDirtyKb) / 1024.0;

        return new double[] {rssMb, dirtyMb};
    }
}
