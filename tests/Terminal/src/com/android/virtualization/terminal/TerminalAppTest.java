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

import static org.junit.Assert.assertTrue;

import android.app.Instrumentation;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;

import androidx.test.InstrumentationRegistry;
import androidx.test.runner.AndroidJUnit4;

import com.android.microdroid.test.common.MetricsProcessor;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;

import java.io.IOException;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

@RunWith(AndroidJUnit4.class)
public class TerminalAppTest {
    private Instrumentation mInstr;
    private Context mTargetContext;
    private final MetricsProcessor mMetricsProc = new MetricsProcessor("avf_perf/terminal/");

    @Before
    public void setup() {
        mInstr = InstrumentationRegistry.getInstrumentation();
        mTargetContext = mInstr.getTargetContext();
        installVmImage();
    }

    private void installVmImage() {
        final long INSTALL_TIMEOUT_MILLIS = 300_000; // 5 min

        Intent intent = new Intent(mTargetContext, InstallerActivity.class);
        intent.setFlags(Intent.FLAG_ACTIVITY_NEW_TASK);

        if (mInstr.startActivitySync(intent) instanceof InstallerActivity activity) {
            assertTrue(
                    "Failed to install VM image",
                    activity.waitForInstallCompleted(INSTALL_TIMEOUT_MILLIS));
        }
    }

    @Test
    public void boot() throws Exception {
        final long BOOT_TIMEOUT_MILLIS = 30_000; // 30 sec

        Intent intent = new Intent(mTargetContext, MainActivity.class);
        intent.setFlags(Intent.FLAG_ACTIVITY_NEW_TASK);

        long start = System.currentTimeMillis();
        if (mInstr.startActivitySync(intent) instanceof MainActivity activity) {
            assertTrue("Failed to boot in 30s", activity.waitForBootCompleted(BOOT_TIMEOUT_MILLIS));
        }
        long delay = System.currentTimeMillis() - start;

        // TODO: measure multiple times?
        List<Long> measurements = new ArrayList<>();
        measurements.add(delay);
        Map<String, Double> stats = mMetricsProc.computeStats(measurements, "boot", "ms");
        Bundle bundle = new Bundle();
        for (Map.Entry<String, Double> entry : stats.entrySet()) {
            bundle.putDouble(entry.getKey(), entry.getValue());
        }
        mInstr.sendStatus(0, bundle);
    }

    @After
    public void tearDown() throws IOException {
        InstalledImage.getDefault(mTargetContext).uninstallFully();
    }
}
