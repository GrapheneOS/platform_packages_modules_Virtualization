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

import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.log.LogUtil.CLog;
import com.android.tradefed.util.Pair;
import com.android.tradefed.util.SimpleStats;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.InputStreamReader;
import java.text.ParseException;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Deque;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.zip.GZIPInputStream;

import javax.annotation.Nonnull;

class KvmHypEvent {
    public final int cpu;
    public final double timestamp;
    public final String name;
    public final String args;
    public final boolean valid;

    private static final Pattern LOST_EVENT_PATTERN =
            Pattern.compile("^CPU:[0-9]* \\[LOST ([0-9]*) EVENTS\\]");

    public KvmHypEvent(String str) {
        Matcher matcher = LOST_EVENT_PATTERN.matcher(str);
        if (matcher.find()) throw new OutOfMemoryError("Lost " + matcher.group(1) + " events");

        Pattern pattern = Pattern.compile("^\\[([0-9]*)\\][ \t]*([0-9]*\\.[0-9]*): (\\S+)\\s*(.*)");

        matcher = pattern.matcher(str);
        if (!matcher.find()) {
            valid = false;
            cpu = 0;
            timestamp = 0;
            name = "";
            args = "";
            CLog.w("Failed to parse hyp event: " + str);
            return;
        }

        cpu = Integer.parseInt(matcher.group(1));
        timestamp = Double.parseDouble(matcher.group(2));
        name = matcher.group(3);
        args = matcher.group(4);
        valid = true;
    }

    public String toString() {
        return String.format("[%03d]\t%f: %s %s", cpu, timestamp, name, args);
    }
}

class KvmHypDurationStats {
    public Pair<KvmHypEvent, KvmHypEvent> maxSection;
    public SimpleStats mStats;

    public KvmHypDurationStats() {
        mStats = new SimpleStats();
    }

    private boolean isValidEvent(KvmHypEvent e) {
        return e.name.equals("hyp_enter") || e.name.equals("hyp_exit");
    }

    private double sectionDuration(Pair<KvmHypEvent, KvmHypEvent> section) {
        return section.second.timestamp - section.first.timestamp;
    }

    public void add(KvmHypEvent start, KvmHypEvent end) throws Exception {
        if (start.cpu != end.cpu
                || start.timestamp > end.timestamp
                || !isValidEvent(start)
                || !isValidEvent(end)) {

            throw new Exception("Unexpected events: " + start + " -> " + end);
        }

        double duration = end.timestamp - start.timestamp;
        mStats.add(duration);

        if (maxSection == null || sectionDuration(maxSection) < duration) {
            maxSection = Pair.create(start, end);
        }
    }
}

/** This class provides utilities to interact with the hyp tracing subsystem */
public final class KvmHypTracer {

    private static final int DEFAULT_BUF_SIZE_KB = 4 * 1024;

    private final String mHypTracingRoot;
    private final CommandRunner mRunner;
    private final ITestDevice mDevice;
    private final int mNrCpus;
    private final String mHypEvents[];

    private final ArrayList<File> mTraces;

    private KvmHypDurationStats mDurationStats;

    private static String getHypTracingRoot(ITestDevice device) throws Exception {
        String legacy = "/sys/kernel/tracing/hyp/";
        String path = "/sys/kernel/tracing/hypervisor/";

        if (device.doesFileExist(path)) {
            return path;
        }

        if (device.doesFileExist(legacy)) {
            return legacy;
        }

        throw new Exception("Hypervisor tracing not found");
    }

    private static String getHypEventsDir(String root) {
        if (root.endsWith("/hypervisor/")) return "events/hypervisor/";

        return "events/hyp/";
    }

    public static boolean isSupported(ITestDevice device, String[] events) throws Exception {
        String dir;

        try {
            dir = getHypTracingRoot(device);
            dir += getHypEventsDir(dir);
        } catch (Exception e) {
            return false;
        }

        for (String event : events) {
            if (!device.doesFileExist(dir + event + "/enable")) return false;
        }
        return true;
    }

    public KvmHypTracer(@Nonnull ITestDevice device, String[] events) throws Exception {
        assertWithMessage("Hypervisor events " + String.join(",", events) + " not supported")
                .that(isSupported(device, events))
                .isTrue();

        mHypTracingRoot = getHypTracingRoot(device);
        mDevice = device;
        mRunner = new CommandRunner(mDevice);
        mTraces = new ArrayList<File>();
        mNrCpus = Integer.parseInt(mRunner.run("nproc"));
        mHypEvents = events;
    }

    private void setNode(String node, int val) throws Exception {
        mRunner.run("echo " + val + " > " + mHypTracingRoot + node);
    }

    private void setNodeString(String node, String val) throws Exception {
        mRunner.run("echo '" + val + "' > " + mHypTracingRoot + node);
    }

    public String run(String payload_cmd, String[] notrace, int buffer_size) throws Exception {
        mTraces.clear();

        setNode("tracing_on", 0);
        mRunner.run("echo 0 | tee " + mHypTracingRoot + "events/*/*/enable");
        setNode("buffer_size_kb", buffer_size);

        for (String event : mHypEvents) {
            setNode(getHypEventsDir(mHypTracingRoot) + event + "/enable", 1);
        }

        if (hasEvent("func") || hasEvent("func_ret")) {
            setNodeString("set_ftrace_filter", "*");

            for (String func : notrace) {
                setNodeString("set_ftrace_notrace", func);
            }
        }

        setNode("trace", 0);

        /* Cat each per-cpu trace_pipe in its own tmp file in the background */
        String cmd = "cd " + mHypTracingRoot + ";";
        String trace_pipes[] = new String[mNrCpus];
        for (int i = 0; i < mNrCpus; i++) {
            /* Toybox's mktemp does not support suffix */
            trace_pipes[i] =
                    mRunner.run(
                            "FILE=$(mktemp -t trace_pipe.cpu"
                                    + i
                                    + ".XXXXXXXXXX) && mv $FILE{,.gz} && echo $FILE.gz");
            cmd += "cat per_cpu/cpu" + i + "/trace_pipe | gzip -c > " + trace_pipes[i] + " &";
            cmd += "CPU" + i + "_TRACE_PIPE_PID=$(lsof /proc/$!/fd/0 -t | head -n 1);";
        }

        String cmd_script = mRunner.run("mktemp -t cmd_script.XXXXXXXXXX");
        mRunner.run("echo '" + payload_cmd + "' > " + cmd_script);

        /* Run the payload with tracing enabled */
        cmd += "echo 1 > tracing_on;";
        String cmd_stdout = mRunner.run("mktemp -t cmd_stdout.XXXXXXXXXX");
        cmd += "sh " + cmd_script + " > " + cmd_stdout + ";";
        cmd += "echo 0 > tracing_on;";

        /* Wait for cat to finish reading the pipe interface before killing it */
        for (int i = 0; i < mNrCpus; i++) {
            cmd +=
                    "while $(test '$(ps -o S -p $CPU"
                            + i
                            + "_TRACE_PIPE_PID | tail -n 1)' = 'R'); do sleep 1; done;";
            cmd += "kill -2 $CPU" + i + "_TRACE_PIPE_PID;";
        }
        cmd += "wait";

        /*
         * The whole thing runs in a single command for simplicity as `adb
         * shell` doesn't play well with subprocesses outliving their parent,
         * and cat-ing a trace_pipe is blocking, so doing so from separate Java
         * threads wouldn't be much easier as we would need to actively kill
         * them too.
         */
        mRunner.run(cmd);

        mRunner.run("rm -f " + cmd_script);

        for (String t : trace_pipes) {
            File trace = mDevice.pullFile(t);
            assertNotNull(trace);
            mTraces.add(trace);
            mRunner.run("rm -f " + t);
        }

        String res = mRunner.run("cat " + cmd_stdout);
        mRunner.run("rm -f " + cmd_stdout);
        return res;
    }

    public String run(String payload_cmd) throws Exception {
        return run(payload_cmd, new String[0], DEFAULT_BUF_SIZE_KB);
    }

    private boolean hasEvent(String event) {
        return Arrays.asList(mHypEvents).contains(event);
    }

    private boolean hasEvents(String[] events) {
        for (String event : events) {
            if (!hasEvent(event)) return false;
        }

        return true;
    }

    private KvmHypEvent getNextEvent(BufferedReader br) throws Exception {
        KvmHypEvent event;
        String l;

        if ((l = br.readLine()) == null) return null;

        event = new KvmHypEvent(l);
        if (!event.valid) return null;

        return event;
    }

    private BufferedReader openTrace(File trace) throws Exception {
        return new BufferedReader(
                new InputStreamReader(new GZIPInputStream(new FileInputStream(trace))));
    }

    private void updateDurationStats() throws Exception {
        if (mDurationStats != null) return;

        String[] reqEvents = {"hyp_enter", "hyp_exit"};
        KvmHypDurationStats stats = new KvmHypDurationStats();

        assertWithMessage("KvmHypTracer() is missing events " + String.join(",", reqEvents))
                .that(hasEvents(reqEvents))
                .isTrue();

        for (File trace : mTraces) {
            try (BufferedReader br = openTrace(trace)) {
                KvmHypEvent event, prevEvent = null;

                while ((event = getNextEvent(br)) != null) {
                    int cpu = event.cpu;
                    if (cpu < 0 || cpu >= mNrCpus)
                        throw new ParseException("Incorrect CPU number: " + cpu, 0);

                    double cur = event.timestamp;
                    if (prevEvent != null && prevEvent.timestamp > cur) {
                        throw new ParseException("Time must not go backward: " + cur, 0);
                    }

                    if (prevEvent != null && prevEvent.name.equals(event.name)) {
                        throw new ParseException(
                                "Hyp event found twice in a row: " + trace + " - " + event, 0);
                    }

                    switch (event.name) {
                        case "hyp_exit":
                            if (prevEvent != null && prevEvent.name.equals("hyp_enter")) {
                                stats.add(prevEvent, event);
                            }
                            break;
                        case "hyp_enter":
                            break;
                        default:
                            continue;
                    }

                    prevEvent = event;
                }
            }
        }

        mDurationStats = stats;
    }

    public SimpleStats getDurationStats() throws Exception {
        updateDurationStats();
        return mDurationStats.mStats;
    }

    public Pair<Double, Double> getMaxDurationSection() throws Exception {
        updateDurationStats();
        return Pair.create(
                mDurationStats.maxSection.first.timestamp,
                mDurationStats.maxSection.second.timestamp);
    }

    public int getMaxDurationCpu() throws Exception {
        updateDurationStats();
        return mDurationStats.maxSection.first.cpu;
    }

    public List<Integer> getPsciMemProtect() throws Exception {
        String[] reqEvents = {"psci_mem_protect"};
        List<Integer> psciMemProtect = new ArrayList<>();

        assertWithMessage("KvmHypTracer() is missing events " + String.join(",", reqEvents))
                .that(hasEvents(reqEvents))
                .isTrue();

        BufferedReader[] brs = new BufferedReader[mTraces.size()];
        KvmHypEvent[] next = new KvmHypEvent[mTraces.size()];

        for (int i = 0; i < mTraces.size(); i++) {
            brs[i] = openTrace(mTraces.get(i));
            next[i] = getNextEvent(brs[i]);
        }

        try {
            while (true) {
                double oldest = Double.MAX_VALUE;
                int oldestIdx = -1;

                for (int i = 0; i < mTraces.size(); i++) {
                    if ((next[i] != null) && (next[i].timestamp < oldest)) {
                        oldest = next[i].timestamp;
                        oldestIdx = i;
                    }
                }

                if (oldestIdx < 0) break;

                Pattern pattern = Pattern.compile("count=([0-9]*) was=([0-9]*)");
                Matcher matcher = pattern.matcher(next[oldestIdx].args);
                if (!matcher.find()) {
                    throw new ParseException(
                            "Unexpected psci_mem_protect event: " + next[oldestIdx], 0);
                }

                int count = Integer.parseInt(matcher.group(1));
                int was = Integer.parseInt(matcher.group(2));

                if (psciMemProtect.isEmpty()) {
                    psciMemProtect.add(was);
                }

                psciMemProtect.add(count);
                next[oldestIdx] = getNextEvent(brs[oldestIdx]);
            }
        } finally {
            for (int i = 0; i < mTraces.size(); i++) brs[i].close();
        }

        return psciMemProtect;
    }

    public Map<String, Double> getFuncDurations(double start, double end, int cpu)
            throws Exception {
        String[] reqEvents = {"func", "func_ret"};
        Map<String, Double> funcDurations = new HashMap<String, Double>();

        assertWithMessage("KvmHypTracer() is missing events " + String.join(",", reqEvents))
                .that(hasEvents(reqEvents))
                .isTrue();

        for (File trace : mTraces) {
            try (BufferedReader br = openTrace(trace)) {
                KvmHypEvent event = getNextEvent(br);
                if (event == null || event.cpu != cpu) continue;

                Deque<Pair<String, Double>> stack = new ArrayDeque<>();
                do {
                    if (event.timestamp < start) continue;
                    if (event.timestamp > end) break;

                    Pair<String, Double> prev = stack.peekFirst();
                    String func = event.args.split(" ")[0];

                    switch (event.name) {
                        case "func":
                            stack.push(Pair.create(func, event.timestamp));
                            break;
                        case "func_ret":
                            if (prev == null) break;

                            if (!prev.first.equals(func)) {
                                throw new Exception(
                                        "Event " + event + " does not match '" + prev.first + "'");
                            }

                            funcDurations.put(
                                    func,
                                    funcDurations.getOrDefault(func, Double.valueOf(0))
                                            + event.timestamp
                                            - prev.second);

                            stack.pop();
                            break;
                        default:
                            break;
                    }
                } while ((event = getNextEvent(br)) != null);
            }
        }

        return funcDurations;
    }
}
