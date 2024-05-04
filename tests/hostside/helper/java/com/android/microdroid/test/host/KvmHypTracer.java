/*
 * Copyright (C) 2023 The Android Open Source Project
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

package com.android.microdroid.test.host;

import static com.google.common.truth.Truth.assertWithMessage;
import static org.junit.Assert.assertNotNull;

import com.android.microdroid.test.host.CommandRunner;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.log.LogUtil.CLog;
import com.android.tradefed.util.SimpleStats;

import java.io.File;
import java.io.FileReader;
import java.io.BufferedReader;
import java.text.ParseException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import javax.annotation.Nonnull;

/** This class provides utilities to interact with the hyp tracing subsystem */
public final class KvmHypTracer {

    private static final String HYP_TRACING_ROOT = "/sys/kernel/tracing/hyp/";
    private static final String HYP_EVENTS[] = { "hyp_enter", "hyp_exit" };
    private static final int DEFAULT_BUF_SIZE_KB = 4 * 1024;
    private static final Pattern LOST_EVENT_PATTERN = Pattern.compile(
            "^CPU:[0-9]* \\[LOST ([0-9]*) EVENTS\\]");
    private static final Pattern EVENT_PATTERN = Pattern.compile(
            "^\\[([0-9]*)\\][ \t]*([0-9]*\\.[0-9]*): (" + String.join("|", HYP_EVENTS) + ") (.*)");

    private final CommandRunner mRunner;
    private final ITestDevice mDevice;
    private final int mNrCpus;

    private final ArrayList<File> mTraces;

    private void setNode(String node, int val) throws Exception {
        mRunner.run("echo " + val + " > " + HYP_TRACING_ROOT + node);
    }

    private static String eventDir(String event) {
        return "events/hyp/" + event + "/";
    }

    public static boolean isSupported(ITestDevice device) throws Exception {
        for (String event: HYP_EVENTS) {
            if (!device.doesFileExist(HYP_TRACING_ROOT + eventDir(event) + "/enable"))
                return false;
        }
        return true;
    }

    public KvmHypTracer(@Nonnull ITestDevice device) throws Exception {
        assertWithMessage("Hypervisor tracing not supported")
                .that(isSupported(device)).isTrue();

        mDevice = device;
        mRunner = new CommandRunner(mDevice);
        mTraces = new ArrayList<File>();
        mNrCpus = Integer.parseInt(mRunner.run("nproc"));
    }

    public String run(String payload_cmd) throws Exception {
        mTraces.clear();

        setNode("tracing_on", 0);
        mRunner.run("echo 0 | tee " + HYP_TRACING_ROOT + "events/*/*/enable");
        setNode("buffer_size_kb", DEFAULT_BUF_SIZE_KB);
        for (String event: HYP_EVENTS)
            setNode(eventDir(event) + "/enable", 1);
        setNode("trace", 0);

        /* Cat each per-cpu trace_pipe in its own tmp file in the background */
        String cmd = "cd " + HYP_TRACING_ROOT + ";";
        String trace_pipes[] = new String[mNrCpus];
        for (int i = 0; i < mNrCpus; i++) {
            trace_pipes[i] = mRunner.run("mktemp -t trace_pipe.cpu" + i + ".XXXXXXXXXX");
            cmd += "cat per_cpu/cpu" + i + "/trace_pipe > " + trace_pipes[i] + " &";
            cmd += "CPU" + i + "_TRACE_PIPE_PID=$!;";
        }

        /* Run the payload with tracing enabled */
        cmd += "echo 1 > tracing_on;";
        String cmd_stdout = mRunner.run("mktemp -t cmd_stdout.XXXXXXXXXX");
        cmd += payload_cmd + " > " + cmd_stdout + ";";
        cmd += "echo 0 > tracing_on;";

        /* Actively kill the cat subprocesses as trace_pipe is blocking */
        for (int i = 0; i < mNrCpus; i++)
            cmd += "kill -9 $CPU" + i + "_TRACE_PIPE_PID;";
        cmd += "wait";

        /*
         * The whole thing runs in a single command for simplicity as `adb
         * shell` doesn't play well with subprocesses outliving their parent,
         * and cat-ing a trace_pipe is blocking, so doing so from separate Java
         * threads wouldn't be much easier as we would need to actively kill
         * them too.
         */
        mRunner.run(cmd);

        for (String t: trace_pipes) {
            File trace = mDevice.pullFile(t);
            assertNotNull(trace);
            mTraces.add(trace);
            mRunner.run("rm -f " + t);
        }

        String res = mRunner.run("cat " + cmd_stdout);
        mRunner.run("rm -f " + cmd_stdout);
        return res;
    }

    public SimpleStats getDurationStats() throws Exception {
        SimpleStats stats = new SimpleStats();

        for (File trace: mTraces) {
            BufferedReader br = new BufferedReader(new FileReader(trace));
            double last = 0.0, hyp_enter = 0.0;
            String l, prev_event = "";
            while ((l = br.readLine()) != null) {
                Matcher matcher = LOST_EVENT_PATTERN.matcher(l);
                if (matcher.find())
                    throw new OutOfMemoryError("Lost " + matcher.group(1) + " events");

                matcher = EVENT_PATTERN.matcher(l);
                if (!matcher.find()) {
                    CLog.w("Failed to parse hyp event: " + l);
                    continue;
                }

                int cpu = Integer.parseInt(matcher.group(1));
                if (cpu < 0 || cpu >= mNrCpus)
                    throw new ParseException("Incorrect CPU number: " + cpu, 0);

                double cur = Double.parseDouble(matcher.group(2));
                if (cur < last)
                    throw new ParseException("Time must not go backward: " + cur, 0);
                last = cur;

                String event = matcher.group(3);
                if (event.equals(prev_event)) {
                    throw new ParseException("Hyp event found twice in a row: " + trace + " - " + l,
                                             0);
                }

                switch (event) {
                    case "hyp_exit":
                        if (prev_event.equals("hyp_enter"))
                            stats.add(cur - hyp_enter);
                        break;
                    case "hyp_enter":
                        hyp_enter = cur;
                        break;
                    default:
                        throw new ParseException("Unexpected line in trace" + l, 0);
                }
                prev_event = event;
            }
        }

        return stats;
    }
}
