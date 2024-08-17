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

package com.android.microdroid.test.common;

import java.io.IOException;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;
import java.util.stream.IntStream;

/** This class provides process utility for both device tests and host tests. */
public final class ProcessUtil {
    private static final String CROSVM_BIN = "/apex/com.android.virt/bin/crosvm";
    private static final String VIRTMGR_BIN = "/apex/com.android.virt/bin/virtmgr";

    /** A memory map entry from /proc/{pid}/smaps */
    public static class SMapEntry {
        public String name;
        public Map<String, Long> metrics;

        @Override
        public String toString() {
            StringBuilder sb = new StringBuilder();
            sb.append("name: " + name + "\n");
            metrics.forEach(
                    (k, v) -> {
                        sb.append("  " + k + ": " + v + "\n");
                    });
            return sb.toString();
        }
    }

    /** Gets metrics key and values mapping of specified process id */
    public static List<SMapEntry> getProcessSmaps(int pid, Function<String, String> shellExecutor)
            throws IOException {
        String path = "/proc/" + pid + "/smaps";
        return parseMemoryInfo(shellExecutor.apply("cat " + path));
    }

    /** Gets metrics key and values mapping of specified process id */
    public static Map<String, Long> getProcessSmapsRollup(
            int pid, Function<String, String> shellExecutor) throws IOException {
        String path = "/proc/" + pid + "/smaps_rollup";
        List<SMapEntry> entries = parseMemoryInfo(shellExecutor.apply("cat " + path + " || true"));
        if (entries.size() > 1) {
            throw new RuntimeException(
                    "expected at most one entry in smaps_rollup, got " + entries.size());
        }
        if (entries.size() == 1) {
            return entries.get(0).metrics;
        }
        return new HashMap<String, Long>();
    }

    /** Gets global memory metrics key and values mapping */
    public static Map<String, Long> getProcessMemoryMap(Function<String, String> shellExecutor)
            throws IOException {
        // The input file of parseMemoryInfo need a header string as the key of output entries.
        // /proc/meminfo doesn't have this line so add one as the key.
        String header = "device memory info\n";
        List<SMapEntry> entries =
                parseMemoryInfo(header + shellExecutor.apply("cat /proc/meminfo"));
        if (entries.size() != 1) {
            throw new RuntimeException(
                    "expected one entry in /proc/meminfo, got " + entries.size());
        }
        return entries.get(0).metrics;
    }

    /** Gets process id and process name mapping of the device */
    public static Map<Integer, String> getProcessMap(Function<String, String> shellExecutor)
            throws IOException {
        Map<Integer, String> processMap = new HashMap<>();
        for (String ps : skipFirstLine(shellExecutor.apply("ps -Ao PID,NAME")).split("\n")) {
            // Each line is '<pid> <name>'.
            // EX : 11424 dex2oat64
            ps = ps.trim();
            if (ps.length() == 0) {
                continue;
            }
            int space = ps.indexOf(" ");
            String pName = ps.substring(space + 1);
            int pId = Integer.parseInt(ps.substring(0, space));
            processMap.put(pId, pName);
        }

        return processMap;
    }

    private static IntStream getChildProcesses(
            int pid, String cmdlineFilter, Function<String, String> shellExecutor) {
        String cmd = "pgrep -P " + pid;
        if (cmdlineFilter != null) {
            cmd += " -f " + cmdlineFilter;
        }
        return shellExecutor.apply(cmd).trim().lines().mapToInt(Integer::parseInt);
    }

    private static int getSingleChildProcess(
            int parentPid, String cmdlineFilter, Function<String, String> shellExecutor) {
        int[] pids = getChildProcesses(parentPid, cmdlineFilter, shellExecutor).toArray();
        if (pids.length == 0) {
            throw new IllegalStateException("No process found for " + cmdlineFilter);
        } else if (pids.length > 1) {
            throw new IllegalStateException("More than one process found for " + cmdlineFilter);
        }
        return pids[0];
    }

    public static int getVirtmgrPid(int parentPid, Function<String, String> shellExecutor) {
        return getSingleChildProcess(parentPid, VIRTMGR_BIN, shellExecutor);
    }

    public static int getCrosvmPid(int parentPid, Function<String, String> shellExecutor) {
        int virtmgrPid = getVirtmgrPid(parentPid, shellExecutor);
        return getSingleChildProcess(virtmgrPid, CROSVM_BIN, shellExecutor);
    }

    // To ensures that only one object is created at a time.
    private ProcessUtil() {}

    private static List<SMapEntry> parseMemoryInfo(String file) {
        List<SMapEntry> entries = new ArrayList<SMapEntry>();
        for (String line : file.split("\n")) {
            line = line.trim();
            if (line.length() == 0) {
                continue;
            }
            // Each line is '<metrics>:        <number> kB'.
            // EX : Pss_Anon:        70712 kB
            // EX : Active(file):     5792 kB
            // EX : ProtectionKey:       0
            if (line.matches("[\\w()]+:\\s+.*")) {
                if (entries.size() == 0) {
                    throw new RuntimeException("unexpected line: " + line);
                }
                if (line.endsWith(" kB")) line = line.substring(0, line.length() - 3);
                String[] elems = line.split(":");
                String name = elems[0].trim();
                try {
                    entries.get(entries.size() - 1)
                            .metrics
                            .put(name, Long.parseLong(elems[1].trim()));
                } catch (java.lang.NumberFormatException e) {
                    // Some entries, like "VmFlags", aren't numbers, just ignore.
                }
                continue;
            }
            // Parse the header and create a new entry for it.
            // Some header examples:
            //     7f644098a000-7f644098c000 rw-p 00000000 00:00 0
            //     00400000-0048a000 r-xp 00000000 fd:03 960637   /bin/bash
            //     75e42af000-75f42af000 rw-s 00000000 00:01 235  /memfd:crosvm_guest (deleted)
            SMapEntry entry = new SMapEntry();
            String[] parts = line.split("\\s+", 6);
            if (parts.length >= 6) {
                entry.name = parts[5];
            } else {
                entry.name = "";
            }
            entry.metrics = new HashMap<String, Long>();
            entries.add(entry);
        }
        return entries;
    }

    private static String skipFirstLine(String str) {
        int index = str.indexOf("\n");
        return (index < 0) ? "" : str.substring(index + 1);
    }
}
