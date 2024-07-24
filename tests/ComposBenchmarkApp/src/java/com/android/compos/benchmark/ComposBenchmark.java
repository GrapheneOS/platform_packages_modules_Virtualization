/*
 * Copyright (C) 2022 The Android Open Source Project
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
package com.android.compos.benchmark;

import static androidx.test.platform.app.InstrumentationRegistry.getInstrumentation;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.TruthJUnit.assume;

import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Instrumentation;
import android.os.Bundle;
import android.util.Log;

import com.android.microdroid.test.common.MetricsProcessor;
import com.android.microdroid.test.common.ProcessUtil;
import com.android.microdroid.test.device.MicrodroidDeviceTestBase;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.JUnit4;

import java.io.IOException;
import java.sql.Timestamp;
import java.text.DateFormat;
import java.text.ParseException;
import java.text.SimpleDateFormat;
import java.util.ArrayList;
import java.util.Date;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

@RunWith(JUnit4.class)
public class ComposBenchmark extends MicrodroidDeviceTestBase {
    private static final String TAG = "ComposBenchmark";
    private static final int BUFFER_SIZE = 1024;
    private static final int ROUND_COUNT = 5;
    private static final double NANOS_IN_SEC = 1_000_000_000.0;
    private static final String METRIC_PREFIX = getMetricPrefix() + "compos/";

    private final MetricsProcessor mMetricsProcessor = new MetricsProcessor(METRIC_PREFIX);

    private Instrumentation mInstrumentation;

    @Before
    public void setup() {
        mInstrumentation = getInstrumentation();
        mInstrumentation.getUiAutomation().adoptShellPermissionIdentity();
    }

    @After
    public void tearDown() {
        mInstrumentation.getUiAutomation().dropShellPermissionIdentity();
    }

    @Test
    public void testHostCompileTime() throws Exception {
        final String command = "/apex/com.android.art/bin/odrefresh --force-compile";

        final List<Double> compileTimes = new ArrayList<>(ROUND_COUNT);
        // The mapping is <memory metrics name> -> <all rounds value list>.
        // EX : pss -> [10, 20, 30, ........]
        final Map<String, List<Long>> processMemory = new HashMap<>();

        for (int round = 0; round < ROUND_COUNT; ++round) {

            GetMetricsRunnable getMetricsRunnable =
                    new GetMetricsRunnable("dex2oat64", processMemory);
            Thread threadGetMetrics = new Thread(getMetricsRunnable);

            threadGetMetrics.start();

            Timestamp beforeCompileLatestTime = getLatestDex2oatSuccessTime();
            Long compileStartTime = System.nanoTime();
            executeCommand(command);
            Long compileEndTime = System.nanoTime();
            Timestamp afterCompileLatestTime = getLatestDex2oatSuccessTime();

            assertNotNull(afterCompileLatestTime);
            assertTrue(
                    beforeCompileLatestTime == null
                            || beforeCompileLatestTime.before(afterCompileLatestTime));

            double elapsedSec = (compileEndTime - compileStartTime) / NANOS_IN_SEC;
            Log.i(TAG, "Compile time in host took " + elapsedSec + "s");
            getMetricsRunnable.stop();

            Log.i(TAG, "Waits for thread finish");
            threadGetMetrics.join();
            Log.i(TAG, "Thread is finish");

            compileTimes.add(elapsedSec);
        }

        reportMetric("host_compile_time", "s", compileTimes);

        reportAggregatedMetric(processMemory, "host_compile_dex2oat64_", "kB");
    }

    @Test
    public void testGuestCompileTime() throws Exception {
        assume().withMessage("Skip on CF; too slow").that(isCuttlefish()).isFalse();
        final String command = "/apex/com.android.compos/bin/composd_cmd test-compile";

        final List<Double> compileTimes = new ArrayList<>(ROUND_COUNT);
        // The mapping is <memory metrics name> -> <all rounds value list>.
        // EX : pss -> [10, 20, 30, ........]
        final Map<String, List<Long>> processMemory = new HashMap<>();

        for (int round = 0; round < ROUND_COUNT; ++round) {

            GetMetricsRunnable getMetricsRunnable = new GetMetricsRunnable("crosvm", processMemory);
            Thread threadGetMetrics = new Thread(getMetricsRunnable);

            threadGetMetrics.start();

            Long compileStartTime = System.nanoTime();
            String output = runInShellWithStderr(TAG, mInstrumentation.getUiAutomation(), command);
            Long compileEndTime = System.nanoTime();
            assertThat(output).containsMatch("All Ok");
            double elapsedSec = (compileEndTime - compileStartTime) / NANOS_IN_SEC;
            Log.i(TAG, "Compile time in guest took " + elapsedSec + "s");
            getMetricsRunnable.stop();

            Log.i(TAG, "Waits for thread finish");
            threadGetMetrics.join();
            Log.i(TAG, "Thread is finish");

            compileTimes.add(elapsedSec);
        }

        reportMetric("guest_compile_time", "s", compileTimes);

        reportAggregatedMetric(processMemory, "guest_compile_crosvm_", "kB");
    }

    private Timestamp getLatestDex2oatSuccessTime()
            throws InterruptedException, IOException, ParseException {
        final String command = "logcat -d -e dex2oat";
        String output = executeCommand(command);
        String latestTime = null;

        for (String line : output.split("[\r\n]+")) {
            Pattern pattern = Pattern.compile("dex2oat64: dex2oat took");
            Matcher matcher = pattern.matcher(line);
            if (matcher.find()) {
                latestTime = line.substring(0, 18);
            }
        }

        if (latestTime == null) {
            return null;
        }

        DateFormat formatter = new SimpleDateFormat("MM-dd hh:mm:ss.SSS");
        Date date = formatter.parse(latestTime);
        Timestamp timeStampDate = new Timestamp(date.getTime());

        return timeStampDate;
    }

    private void reportMetric(String name, String unit, List<? extends Number> values) {
        Log.d(TAG, "Report metric " + name + "(" + unit + ") : " + values.toString());
        Map<String, Double> stats = mMetricsProcessor.computeStats(values, name, unit);
        Bundle bundle = new Bundle();
        for (Map.Entry<String, Double> entry : stats.entrySet()) {
            bundle.putDouble(entry.getKey(), entry.getValue());
        }
        mInstrumentation.sendStatus(0, bundle);
    }

    private void reportAggregatedMetric(
            Map<String, List<Long>> processMemory, String prefix, String unit) {
        processMemory.forEach((k, v) -> reportMetric(prefix + k, unit, v));
    }

    private String executeCommand(String command) {
        return runInShell(TAG, mInstrumentation.getUiAutomation(), command);
    }

    private class GetMetricsRunnable implements Runnable {
        private final String mProcessName;
        private Map<String, List<Long>> mProcessMemory;
        private AtomicBoolean mStop = new AtomicBoolean(false);

        GetMetricsRunnable(String processName, Map<String, List<Long>> processMemory) {
            this.mProcessName = processName;
            this.mProcessMemory = processMemory;
        }

        void stop() {
            mStop.set(true);
        }

        public void run() {
            while (!mStop.get()) {
                try {
                    updateProcessMemory(mProcessName, mProcessMemory);
                    Thread.sleep(1000);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return;
                } catch (Exception e) {
                    Log.e(TAG, "Get exception : " + e);
                    throw new RuntimeException(e);
                }
            }
        }
    }

    private void updateProcessMemory(String processName, Map<String, List<Long>> processMemory)
            throws Exception {
        for (Map.Entry<Integer, String> process :
                ProcessUtil.getProcessMap(this::executeCommand).entrySet()) {
            int pId = process.getKey();
            String pName = process.getValue();
            if (pName.equalsIgnoreCase(processName)) {
                for (Map.Entry<String, Long> stat :
                        ProcessUtil.getProcessSmapsRollup(pId, this::executeCommand).entrySet()) {
                    Log.i(
                            TAG,
                            "Get running process "
                                    + pName
                                    + " metrics : "
                                    + stat.getKey().toLowerCase()
                                    + '-'
                                    + stat.getValue());
                    processMemory
                            .computeIfAbsent(stat.getKey().toLowerCase(), k -> new ArrayList<>())
                            .add(stat.getValue());
                }
            }
        }
    }
}
