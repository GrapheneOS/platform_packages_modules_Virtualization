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

package com.android.microdroid.benchmark;

import static android.system.virtualmachine.VirtualMachineConfig.CPU_TOPOLOGY_MATCH_HOST;
import static android.system.virtualmachine.VirtualMachineConfig.CPU_TOPOLOGY_ONE_CPU;
import static android.system.virtualmachine.VirtualMachineConfig.DEBUG_LEVEL_FULL;
import static android.system.virtualmachine.VirtualMachineConfig.DEBUG_LEVEL_NONE;

import static androidx.test.platform.app.InstrumentationRegistry.getInstrumentation;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.Truth.assertWithMessage;
import static com.google.common.truth.TruthJUnit.assume;

import android.app.Application;
import android.app.Instrumentation;
import android.content.ComponentCallbacks2;
import android.content.Context;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.ParcelFileDescriptor.AutoCloseInputStream;
import android.os.ParcelFileDescriptor.AutoCloseOutputStream;
import android.os.RemoteException;
import android.system.Os;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineException;
import android.system.virtualmachine.VirtualMachineManager;
import android.util.Log;

import com.android.microdroid.test.common.MetricsProcessor;
import com.android.microdroid.test.common.ProcessUtil;
import com.android.microdroid.test.device.MicrodroidDeviceTestBase;
import com.android.microdroid.testservice.IBenchmarkService;
import com.android.microdroid.testservice.ITestService;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.Timeout;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.OptionalLong;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Function;

@RunWith(Parameterized.class)
public class MicrodroidBenchmarks extends MicrodroidDeviceTestBase {
    private static final String TAG = "MicrodroidBenchmarks";
    private static final String METRIC_NAME_PREFIX = getMetricPrefix() + "microdroid/";
    private static final int IO_TEST_TRIAL_COUNT = 5;
    private static final int TEST_TRIAL_COUNT = 5;
    private static final long ONE_MEBI = 1024 * 1024;

    @Rule public Timeout globalTimeout = Timeout.seconds(300);

    private static final String APEX_ETC_FS = "/apex/com.android.virt/etc/fs/";
    private static final double SIZE_MB = 1024.0 * 1024.0;
    private static final double NANO_TO_MILLI = 1_000_000.0;
    private static final double NANO_TO_MICRO = 1_000.0;
    private static final String MICRODROID_IMG_PREFIX = "microdroid_";
    private static final String MICRODROID_IMG_SUFFIX = ".img";

    @Parameterized.Parameters(name = "protectedVm={0},gki={1}")
    public static Collection<Object[]> params() {
        List<Object[]> ret = new ArrayList<>();
        ret.add(new Object[] {true /* protectedVm */, null /* use microdroid kernel */});
        ret.add(new Object[] {false /* protectedVm */, null /* use microdroid kernel */});
        for (String gki : SUPPORTED_GKI_VERSIONS) {
            ret.add(new Object[] {true /* protectedVm */, gki});
            ret.add(new Object[] {false /* protectedVm */, gki});
        }
        return ret;
    }

    @Parameterized.Parameter(0)
    public boolean mProtectedVm;

    @Parameterized.Parameter(1)
    public String mGki;

    private final MetricsProcessor mMetricsProcessor = new MetricsProcessor(METRIC_NAME_PREFIX);

    private Instrumentation mInstrumentation;

    private boolean mTeardownDebugfs;

    private void setupDebugfs() throws IOException {
        BufferedReader reader = new BufferedReader(new FileReader("/proc/mounts"));

        mTeardownDebugfs =
                !reader.lines().filter(line -> line.startsWith("debugfs ")).findAny().isPresent();

        if (mTeardownDebugfs) {
            executeCommand("mount -t debugfs none /sys/kernel/debug");
        }
    }

    public MicrodroidBenchmarks() throws IOException {
        // See b/325745564#comment28. Calling this method here ensures that threads spawned for
        // @Test methods are with the desired task profile. If this is called in @Before, the task
        // profile may not be set to the test threads because they may be spanwed prior to the
        // execution of the @Before method (though the test methods will be executed after the
        // @Before method).  With this, children of this benchmark process (virtmgr and crosvm) also
        // run in the desired task profile.
        setMaxPerformanceTaskProfile();
    }

    @Before
    public void setup() throws IOException {
        grantPermission(VirtualMachine.MANAGE_VIRTUAL_MACHINE_PERMISSION);
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        prepareTestSetup(mProtectedVm, mGki);
        mInstrumentation = getInstrumentation();
    }

    @After
    public void tearDown() throws IOException {
        if (mTeardownDebugfs) {
            executeCommand("umount /sys/kernel/debug");
        }
    }

    private boolean canBootMicrodroidWithMemory(int mem)
            throws VirtualMachineException, InterruptedException, IOException {
        VirtualMachineConfig normalConfig =
                newVmConfigBuilderWithPayloadBinary("MicrodroidIdleNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .setMemoryBytes(mem * ONE_MEBI)
                        .setShouldUseHugepages(true)
                        .build();

        // returns true if succeeded at least once.
        final int trialCount = 5;
        for (int i = 0; i < trialCount; i++) {
            forceCreateNewVirtualMachine("test_vm_minimum_memory", normalConfig);

            if (tryBootVm(TAG, "test_vm_minimum_memory").payloadStarted) return true;
        }

        return false;
    }

    @Test
    public void testMinimumRequiredRAM()
            throws VirtualMachineException, InterruptedException, IOException {
        assume().withMessage("Skip on CF; too slow").that(isCuttlefish()).isFalse();

        int lo = 16, hi = 512, minimum = 0;
        boolean found = false;

        while (lo <= hi) {
            int mid = (lo + hi) / 2;
            if (canBootMicrodroidWithMemory(mid)) {
                found = true;
                minimum = mid;
                hi = mid - 1;
            } else {
                lo = mid + 1;
            }
        }

        assertThat(found).isTrue();

        Bundle bundle = new Bundle();
        bundle.putInt(METRIC_NAME_PREFIX + "minimum_required_memory", minimum);
        mInstrumentation.sendStatus(0, bundle);
    }

    private static class BootTimeStats {
        private final Map<BootTimeMetric, List<Double>> mData = new HashMap<>();

        public BootTimeStats(int trialCount) {
            for (BootTimeMetric metric : BootTimeMetric.values()) {
                mData.put(metric, new ArrayList<>(trialCount));
            }
        }

        public void collect(BootResult result) {
            for (BootTimeMetric metric : BootTimeMetric.values()) {
                OptionalLong value = result.getBootTimeMetricNanoTime(metric);
                if (value.isPresent()) {
                    mData.get(metric).add(value.getAsLong() / NANO_TO_MILLI);
                }
            }
        }

        public List<Double> get(BootTimeMetric metric) {
            return Collections.unmodifiableList(mData.get(metric));
        }
    }

    private void runBootTimeTest(
            String name,
            boolean fullDebug,
            Function<VirtualMachineConfig.Builder, VirtualMachineConfig.Builder> fnConfig)
            throws VirtualMachineException, InterruptedException, IOException {
        assume().withMessage("Skip on CF; too slow").that(isCuttlefish()).isFalse();

        final int trialCount = 10;

        BootTimeStats stats = new BootTimeStats(trialCount);
        for (int i = 0; i < trialCount; i++) {
            VirtualMachineConfig.Builder builder =
                    newVmConfigBuilderWithPayloadBinary("MicrodroidIdleNativeLib.so")
                            .setShouldBoostUclamp(true)
                            .setShouldUseHugepages(true)
                            .setMemoryBytes(256 * ONE_MEBI)
                            .setDebugLevel(DEBUG_LEVEL_NONE);
            if (fullDebug) {
                builder = builder.setDebugLevel(DEBUG_LEVEL_FULL).setVmOutputCaptured(true);
            }
            VirtualMachineConfig config = fnConfig.apply(builder).build();
            forceCreateNewVirtualMachine(name, config);

            BootResult result = tryBootVm(TAG, name);
            assertThat(result.payloadStarted).isTrue();
            stats.collect(result);
        }

        reportMetrics(stats.get(BootTimeMetric.TOTAL), "boot_time", "ms");
        if (fullDebug) {
            reportMetrics(stats.get(BootTimeMetric.VM_START), "vm_starting_time", "ms");
            reportMetrics(stats.get(BootTimeMetric.BOOTLOADER), "bootloader_time", "ms");
            reportMetrics(stats.get(BootTimeMetric.KERNEL), "kernel_boot_time", "ms");
            reportMetrics(stats.get(BootTimeMetric.USERSPACE), "userspace_boot_time", "ms");
        }
    }

    @Test
    public void testMicrodroidBootTime()
            throws VirtualMachineException, InterruptedException, IOException {
        runBootTimeTest(
                "test_vm_boot_time",
                /* fullDebug */ false,
                (builder) -> builder.setCpuTopology(CPU_TOPOLOGY_ONE_CPU));
    }

    @Test
    public void testMicrodroidHostCpuTopologyBootTime()
            throws VirtualMachineException, InterruptedException, IOException {
        runBootTimeTest(
                "test_vm_boot_time_host_topology",
                /* fullDebug */ false,
                (builder) -> builder.setCpuTopology(CPU_TOPOLOGY_MATCH_HOST));
    }

    @Test
    public void testMicrodroidDebugBootTime()
            throws VirtualMachineException, InterruptedException, IOException {
        runBootTimeTest("test_vm_boot_time_debug", /* fullDebug */ true, (builder) -> builder);
    }

    private void testMicrodroidDebugBootTime_withVendorBase(File vendorDiskImage) throws Exception {
        // TODO(b/325094712): Boot fails with vendor partition in Cuttlefish.
        assume().withMessage("Cuttlefish doesn't support device tree under /proc/device-tree")
                .that(isCuttlefish())
                .isFalse();
        // TODO(b/317567210): Boots fails with vendor partition in HWASAN enabled microdroid
        // after introducing verification based on DT and fstab in microdroid vendor partition.
        assume().withMessage("Boot with vendor partition is failing in HWASAN enabled Microdroid.")
                .that(isHwasan())
                .isFalse();
        assumeFeatureEnabled(VirtualMachineManager.FEATURE_VENDOR_MODULES);
        runBootTimeTest(
                "test_vm_boot_time_debug_with_vendor_partition",
                /* fullDebug */ true,
                (builder) -> builder.setVendorDiskImage(vendorDiskImage));
    }

    @Test
    public void testMicrodroidDebugBootTime_withVendorPartition() throws Exception {
        File vendorDiskImage = new File("/vendor/etc/avf/microdroid/microdroid_vendor.img");
        assume().withMessage("Microdroid vendor image doesn't exist, skip")
                .that(vendorDiskImage.exists())
                .isTrue();
        testMicrodroidDebugBootTime_withVendorBase(vendorDiskImage);
    }

    @Test
    public void testMicrodroidDebugBootTime_withCustomVendorPartition() throws Exception {
        assume().withMessage(
                        "Skip test for protected VM, pvmfw config data doesn't contain any"
                                + " information of test images, such as root digest.")
                .that(mProtectedVm)
                .isFalse();
        File vendorDiskImage =
                new File("/data/local/tmp/microdroid-bench/microdroid_vendor_image.img");
        testMicrodroidDebugBootTime_withVendorBase(vendorDiskImage);
    }

    @Test
    public void testMicrodroidImageSize() throws IOException {
        Bundle bundle = new Bundle();
        for (File file : new File(APEX_ETC_FS).listFiles()) {
            String name = file.getName();

            if (!name.startsWith(MICRODROID_IMG_PREFIX) || !name.endsWith(MICRODROID_IMG_SUFFIX)) {
                continue;
            }

            String base =
                    name.substring(
                            MICRODROID_IMG_PREFIX.length(),
                            name.length() - MICRODROID_IMG_SUFFIX.length());
            String metric = METRIC_NAME_PREFIX + "img_size_" + base + "_MB";
            double size = Files.size(file.toPath()) / SIZE_MB;
            bundle.putDouble(metric, size);
        }
        mInstrumentation.sendStatus(0, bundle);
    }

    @Test
    public void testVsockTransferFromHostToVM() throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_io.json")
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .setShouldBoostUclamp(true)
                        .setShouldUseHugepages(true)
                        .build();
        List<Double> transferRates = new ArrayList<>(IO_TEST_TRIAL_COUNT);

        for (int i = 0; i < IO_TEST_TRIAL_COUNT; ++i) {
            int port = (mProtectedVm ? 5666 : 6666) + i;
            String vmName = "test_vm_io_" + i;
            VirtualMachine vm = forceCreateNewVirtualMachine(vmName, config);
            BenchmarkVmListener.create(new VsockListener(transferRates, port)).runToFinish(TAG, vm);
        }
        reportMetrics(transferRates, "vsock/transfer_host_to_vm", "mb_per_sec");
    }

    @Test
    public void testVirtioBlkSeqReadRate() throws Exception {
        testVirtioBlkReadRate(/* isRand= */ false);
    }

    @Test
    public void testVirtioBlkRandReadRate() throws Exception {
        testVirtioBlkReadRate(/* isRand= */ true);
    }

    private void testVirtioBlkReadRate(boolean isRand) throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_io.json")
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .setShouldUseHugepages(true)
                        .build();
        List<Double> readRates = new ArrayList<>(IO_TEST_TRIAL_COUNT);

        for (int i = 0; i < IO_TEST_TRIAL_COUNT + 1; ++i) {
            if (i == 1) {
                // Clear the first result because when the file was loaded the first time,
                // the data also needs to be loaded from hard drive to host. This is
                // not part of the virtio-blk IO throughput.
                readRates.clear();
            }
            String vmName = "test_vm_io_" + i;
            VirtualMachine vm = forceCreateNewVirtualMachine(vmName, config);
            BenchmarkVmListener.create(new VirtioBlkListener(readRates, isRand))
                    .runToFinish(TAG, vm);
        }
        reportMetrics(
                readRates, isRand ? "virtio-blk/rand_read" : "virtio-blk/seq_read", "mb_per_sec");
    }

    private void reportMetrics(List<Double> metrics, String name, String unit) {
        Map<String, Double> stats = mMetricsProcessor.computeStats(metrics, name, unit);
        Bundle bundle = new Bundle();
        for (Map.Entry<String, Double> entry : stats.entrySet()) {
            bundle.putDouble(entry.getKey(), entry.getValue());
        }
        mInstrumentation.sendStatus(0, bundle);
    }

    private static class VirtioBlkListener implements BenchmarkVmListener.InnerListener {
        private static final String FILENAME = APEX_ETC_FS + "microdroid_super.img";

        private final List<Double> mReadRates;
        private final boolean mIsRand;

        VirtioBlkListener(List<Double> readRates, boolean isRand) {
            mReadRates = readRates;
            mIsRand = isRand;
        }

        @Override
        public void onPayloadReady(VirtualMachine vm, IBenchmarkService benchmarkService)
                throws RemoteException {
            double readRate = benchmarkService.measureReadRate(FILENAME, mIsRand);
            mReadRates.add(readRate);
        }
    }

    private String executeCommand(String command) {
        return runInShell(TAG, mInstrumentation.getUiAutomation(), command);
    }

    private static class CrosvmStats {
        public final long mHostRss;
        public final long mHostPss;
        public final long mGuestRss;
        public final long mGuestPss;

        CrosvmStats(int vmPid, Function<String, String> shellExecutor) {
            try {
                long hostRss = 0;
                long hostPss = 0;
                long guestRss = 0;
                long guestPss = 0;
                boolean hasGuestMaps = false;
                List<ProcessUtil.SMapEntry> smaps =
                        ProcessUtil.getProcessSmaps(vmPid, shellExecutor);
                for (ProcessUtil.SMapEntry entry : smaps) {
                    long rss = entry.metrics.get("Rss");
                    long pss = entry.metrics.get("Pss");
                    if (entry.name.contains("crosvm_guest")) {
                        guestRss += rss;
                        guestPss += pss;
                        hasGuestMaps = true;
                    } else {
                        hostRss += rss;
                        hostPss += pss;
                    }
                }
                if (!hasGuestMaps) {
                    StringBuilder sb = new StringBuilder();
                    for (ProcessUtil.SMapEntry entry : smaps) {
                        sb.append(entry.toString());
                    }
                    throw new IllegalStateException(
                            "found no crosvm_guest smap entry in crosvm process: " + sb);
                }
                mHostRss = hostRss;
                mHostPss = hostPss;
                mGuestRss = guestRss;
                mGuestPss = guestPss;
            } catch (Exception e) {
                Log.e(TAG, "Error inside onPayloadReady():" + e);
                throw new RuntimeException(e);
            }
        }
    }

    private static class KvmVmStats {
        public final long mProtectedHyp;
        public final long mProtectedShared;
        private final Function<String, String> mShellExecutor;
        private static final String KVM_STATS_FS = "/sys/kernel/debug/kvm";

        public static KvmVmStats createIfSupported(
                int vmPid, Function<String, String> shellExecutor) {

            if (!new File(KVM_STATS_FS + "/protected_hyp_mem").exists()) {
                return null;
            }

            return new KvmVmStats(vmPid, shellExecutor);
        }

        KvmVmStats(int vmPid, Function<String, String> shellExecutor) {
            mShellExecutor = shellExecutor;

            try {
                String dir = getKvmVmStatDir(vmPid);

                mProtectedHyp = getKvmVmStat(dir, "protected_hyp_mem");
                mProtectedShared = getKvmVmStat(dir, "protected_shared_mem");

            } catch (Exception e) {
                Log.e(TAG, "Error inside onPayloadReady():" + e);
                throw new RuntimeException(e);
            }
        }

        private String getKvmVmStatDir(int vmPid) {
            String output = mShellExecutor.apply("find " + KVM_STATS_FS + " -type d");

            for (String line : output.split("\n")) {
                if (line.startsWith(KVM_STATS_FS + "/" + Integer.toString(vmPid) + "-")) {
                    return line;
                }
            }

            throw new IllegalStateException("KVM stat folder for PID " + vmPid + " not found");
        }

        private int getKvmVmStat(String dir, String name) throws IOException {
            return Integer.parseInt(mShellExecutor.apply("cat " + dir + "/" + name).trim());
        }
    }

    @Test
    public void testMemoryUsage() throws Exception {
        final String vmName = "test_vm_mem_usage";
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_io.json")
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .setShouldUseHugepages(true)
                        .setMemoryBytes(256 * ONE_MEBI)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine(vmName, config);
        MemoryUsageListener listener = new MemoryUsageListener(this::executeCommand);

        setupDebugfs();

        BenchmarkVmListener.create(listener).runToFinish(TAG, vm);

        assertWithMessage("VM failed to start").that(listener.mCrosvm).isNotNull();

        double mem_overall = 256.0;
        double mem_total = (double) listener.mMemTotal / 1024.0;
        double mem_free = (double) listener.mMemFree / 1024.0;
        double mem_avail = (double) listener.mMemAvailable / 1024.0;
        double mem_buffers = (double) listener.mBuffers / 1024.0;
        double mem_cached = (double) listener.mCached / 1024.0;
        double mem_slab = (double) listener.mSlab / 1024.0;
        double mem_crosvm_host_rss = (double) listener.mCrosvm.mHostRss / 1024.0;
        double mem_crosvm_host_pss = (double) listener.mCrosvm.mHostPss / 1024.0;
        double mem_crosvm_guest_rss = (double) listener.mCrosvm.mGuestRss / 1024.0;
        double mem_crosvm_guest_pss = (double) listener.mCrosvm.mGuestPss / 1024.0;

        double mem_kernel = mem_overall - mem_total;
        double mem_used = mem_total - mem_free - mem_buffers - mem_cached - mem_slab;
        double mem_unreclaimable = mem_total - mem_avail;

        Bundle bundle = new Bundle();
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_kernel_MB", mem_kernel);
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_used_MB", mem_used);
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_buffers_MB", mem_buffers);
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_cached_MB", mem_cached);
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_slab_MB", mem_slab);
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_unreclaimable_MB", mem_unreclaimable);
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_crosvm_host_rss_MB", mem_crosvm_host_rss);
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_crosvm_host_pss_MB", mem_crosvm_host_pss);
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_crosvm_guest_rss_MB", mem_crosvm_guest_rss);
        bundle.putDouble(METRIC_NAME_PREFIX + "mem_crosvm_guest_pss_MB", mem_crosvm_guest_pss);
        if (listener.mKvm != null) {
            double mem_protected_shared = (double) listener.mKvm.mProtectedShared / 1048576.0;
            double mem_protected_hyp = (double) listener.mKvm.mProtectedHyp / 1048576.0;
            bundle.putDouble(METRIC_NAME_PREFIX + "mem_protected_shared_MB", mem_protected_shared);
            bundle.putDouble(METRIC_NAME_PREFIX + "mem_protected_hyp_MB", mem_protected_hyp);
        }
        mInstrumentation.sendStatus(0, bundle);
    }

    private static class MemoryUsageListener implements BenchmarkVmListener.InnerListener {
        MemoryUsageListener(Function<String, String> shellExecutor) {
            mShellExecutor = shellExecutor;
        }

        public final Function<String, String> mShellExecutor;

        public long mMemTotal;
        public long mMemFree;
        public long mMemAvailable;
        public long mBuffers;
        public long mCached;
        public long mSlab;

        public CrosvmStats mCrosvm;
        public KvmVmStats mKvm;

        @Override
        public void onPayloadReady(VirtualMachine vm, IBenchmarkService service)
                throws RemoteException {
            int vmPid = ProcessUtil.getCrosvmPid(Os.getpid(), mShellExecutor);

            mMemTotal = service.getMemInfoEntry("MemTotal");
            mMemFree = service.getMemInfoEntry("MemFree");
            mMemAvailable = service.getMemInfoEntry("MemAvailable");
            mBuffers = service.getMemInfoEntry("Buffers");
            mCached = service.getMemInfoEntry("Cached");
            mSlab = service.getMemInfoEntry("Slab");
            mCrosvm = new CrosvmStats(vmPid, mShellExecutor);
            mKvm = KvmVmStats.createIfSupported(vmPid, mShellExecutor);
        }
    }

    @Test
    public void testMemoryReclaim() throws Exception {
        final String vmName = "test_vm_mem_reclaim";
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_io.json")
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .setShouldUseHugepages(true)
                        .setMemoryBytes(256 * ONE_MEBI)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine(vmName, config);
        MemoryReclaimListener listener =
                new MemoryReclaimListener(this::executeCommand, getContext());
        BenchmarkVmListener.create(listener).runToFinish(TAG, vm);
        assertWithMessage("VM failed to start").that(listener.mPreCrosvm).isNotNull();
        assertWithMessage("Post trim stats not available").that(listener.mPostCrosvm).isNotNull();

        double mem_pre_crosvm_host_rss = (double) listener.mPreCrosvm.mHostRss / 1024.0;
        double mem_pre_crosvm_host_pss = (double) listener.mPreCrosvm.mHostPss / 1024.0;
        double mem_pre_crosvm_guest_rss = (double) listener.mPreCrosvm.mGuestRss / 1024.0;
        double mem_pre_crosvm_guest_pss = (double) listener.mPreCrosvm.mGuestPss / 1024.0;
        double mem_post_crosvm_host_rss = (double) listener.mPostCrosvm.mHostRss / 1024.0;
        double mem_post_crosvm_host_pss = (double) listener.mPostCrosvm.mHostPss / 1024.0;
        double mem_post_crosvm_guest_rss = (double) listener.mPostCrosvm.mGuestRss / 1024.0;
        double mem_post_crosvm_guest_pss = (double) listener.mPostCrosvm.mGuestPss / 1024.0;

        Bundle bundle = new Bundle();
        bundle.putDouble(
                METRIC_NAME_PREFIX + "mem_pre_crosvm_host_rss_MB", mem_pre_crosvm_host_rss);
        bundle.putDouble(
                METRIC_NAME_PREFIX + "mem_pre_crosvm_host_pss_MB", mem_pre_crosvm_host_pss);
        bundle.putDouble(
                METRIC_NAME_PREFIX + "mem_pre_crosvm_guest_rss_MB", mem_pre_crosvm_guest_rss);
        bundle.putDouble(
                METRIC_NAME_PREFIX + "mem_pre_crosvm_guest_pss_MB", mem_pre_crosvm_guest_pss);
        bundle.putDouble(
                METRIC_NAME_PREFIX + "mem_post_crosvm_host_rss_MB", mem_post_crosvm_host_rss);
        bundle.putDouble(
                METRIC_NAME_PREFIX + "mem_post_crosvm_host_pss_MB", mem_post_crosvm_host_pss);
        bundle.putDouble(
                METRIC_NAME_PREFIX + "mem_post_crosvm_guest_rss_MB", mem_post_crosvm_guest_rss);
        bundle.putDouble(
                METRIC_NAME_PREFIX + "mem_post_crosvm_guest_pss_MB", mem_post_crosvm_guest_pss);
        mInstrumentation.sendStatus(0, bundle);
    }

    private static class MemoryReclaimListener implements BenchmarkVmListener.InnerListener {
        MemoryReclaimListener(Function<String, String> shellExecutor, Context applicationCtx) {
            mShellExecutor = shellExecutor;
            mApplication = (Application) applicationCtx;
        }

        public final Function<String, String> mShellExecutor;
        private final Application mApplication;

        public CrosvmStats mPreCrosvm;
        public CrosvmStats mPostCrosvm;

        @Override
        @SuppressWarnings("ReturnValueIgnored")
        public void onPayloadReady(VirtualMachine vm, IBenchmarkService service)
                throws RemoteException {
            int vmPid = ProcessUtil.getCrosvmPid(Os.getpid(), mShellExecutor);

            // Allocate 256MB of anonymous memory. This will fill all guest
            // memory and cause swapping to start.
            service.allocAnonMemory(256);
            mPreCrosvm = new CrosvmStats(vmPid, mShellExecutor);
            // Send a memory trim hint to cause memory reclaim.
            mApplication.onTrimMemory(ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL);
            // Give time for the memory reclaim to do its work.
            try {
                Thread.sleep(isCuttlefish() ? 10000 : 5000);
            } catch (InterruptedException e) {
                Log.e(TAG, "Interrupted sleep:" + e);
                Thread.currentThread().interrupt();
            }
            mPostCrosvm = new CrosvmStats(vmPid, mShellExecutor);
        }
    }

    private static class VsockListener implements BenchmarkVmListener.InnerListener {
        private static final int NUM_BYTES_TO_TRANSFER = 48 * 1024 * 1024;

        private final List<Double> mReadRates;
        private final int mPort;

        VsockListener(List<Double> readRates, int port) {
            mReadRates = readRates;
            mPort = port;
        }

        @Override
        public void onPayloadReady(VirtualMachine vm, IBenchmarkService benchmarkService)
                throws RemoteException {
            AtomicReference<Double> sendRate = new AtomicReference();

            int serverFd = benchmarkService.initVsockServer(mPort);
            new Thread(() -> sendRate.set(runVsockClientAndSendData(vm))).start();
            benchmarkService.runVsockServerAndReceiveData(serverFd, NUM_BYTES_TO_TRANSFER);

            Double rate = sendRate.get();
            if (rate == null) {
                throw new IllegalStateException("runVsockClientAndSendData() failed");
            }
            mReadRates.add(rate);
        }

        private double runVsockClientAndSendData(VirtualMachine vm) {
            try {
                ParcelFileDescriptor fd = vm.connectVsock(mPort);
                double sendRate =
                        IoVsockHostNative.measureSendRate(fd.getFd(), NUM_BYTES_TO_TRANSFER);
                fd.closeWithError("Cannot close socket file descriptor");
                return sendRate;
            } catch (Exception e) {
                Log.e(TAG, "Error inside runVsockClientAndSendData():" + e);
                throw new RuntimeException(e);
            }
        }
    }

    @Test
    public void testRpcBinderLatency() throws Exception {
        final int NUM_WARMUPS = 10;
        final int NUM_REQUESTS = 10_000;

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .setShouldUseHugepages(true)
                        .setShouldBoostUclamp(true)
                        .build();

        List<Double> requestLatencies = new ArrayList<>(IO_TEST_TRIAL_COUNT * NUM_REQUESTS);
        for (int i = 0; i < IO_TEST_TRIAL_COUNT; ++i) {
            VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_latency" + i, config);
            TestResults testResults =
                    runVmTestService(
                            TAG,
                            vm,
                            (ts, tr) -> {
                                // Correctness check
                                tr.mAddInteger = ts.addInteger(123, 456);

                                // Warmup
                                for (int j = 0; j < NUM_WARMUPS; j++) {
                                    ts.addInteger(j, j + 1);
                                }

                                // Count Fibonacci numbers, measure latency.
                                int a = 0;
                                int b = 1;
                                int c;
                                tr.mTimings = new long[NUM_REQUESTS];
                                for (int j = 0; j < NUM_REQUESTS; j++) {
                                    long start = System.nanoTime();
                                    c = ts.addInteger(a, b);
                                    tr.mTimings[j] = System.nanoTime() - start;
                                    a = b;
                                    b = c;
                                }
                            });
            testResults.assertNoException();
            assertThat(testResults.mAddInteger).isEqualTo(579);
            for (long duration : testResults.mTimings) {
                requestLatencies.add((double) duration / NANO_TO_MICRO);
            }
        }
        reportMetrics(requestLatencies, "latency/rpcbinder", "us");
    }

    @Test
    public void testVsockLatency() throws Exception {
        final int NUM_WARMUPS = 10;
        final int NUM_REQUESTS = 10_000;

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .setShouldBoostUclamp(true)
                        .setShouldUseHugepages(true)
                        .build();

        List<Double> requestLatencies = new ArrayList<>(IO_TEST_TRIAL_COUNT * NUM_REQUESTS);
        for (int i = 0; i < IO_TEST_TRIAL_COUNT; ++i) {
            VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_latency" + i, config);
            TestResults testResults =
                    runVmTestService(
                            TAG,
                            vm,
                            (ts, tr) -> {
                                ts.runEchoReverseServer();
                                ParcelFileDescriptor pfd =
                                        vm.connectVsock(ITestService.ECHO_REVERSE_PORT);
                                try (InputStream input = new AutoCloseInputStream(pfd);
                                        OutputStream output = new AutoCloseOutputStream(pfd)) {
                                    BufferedReader reader =
                                            new BufferedReader(new InputStreamReader(input));
                                    Writer writer = new OutputStreamWriter(output);

                                    // Correctness check.
                                    writer.write("hello\n");
                                    writer.flush();
                                    tr.mFileContent = reader.readLine().trim();

                                    // Warmup.
                                    for (int j = 0; j < NUM_WARMUPS; ++j) {
                                        String text = "test" + j + "\n";
                                        writer.write(text);
                                        writer.flush();
                                        reader.readLine();
                                    }

                                    // Measured requests.
                                    tr.mTimings = new long[NUM_REQUESTS];
                                    for (int j = 0; j < NUM_REQUESTS; j++) {
                                        String text = "test" + j + "\n";
                                        long start = System.nanoTime();
                                        writer.write(text);
                                        writer.flush();
                                        reader.readLine();
                                        tr.mTimings[j] = System.nanoTime() - start;
                                    }
                                }
                            });
            testResults.assertNoException();
            assertThat(testResults.mFileContent).isEqualTo("olleh");
            for (long duration : testResults.mTimings) {
                requestLatencies.add((double) duration / NANO_TO_MICRO);
            }
        }
        reportMetrics(requestLatencies, "latency/vsock", "us");
    }

    @Test
    public void testVmKillTime() throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_io.json")
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .setShouldUseHugepages(true)
                        .build();
        List<Double> vmKillTime = new ArrayList<>(TEST_TRIAL_COUNT);

        for (int i = 0; i < TEST_TRIAL_COUNT; ++i) {
            VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_kill_time" + i, config);
            VmEventListener listener =
                    new VmEventListener() {
                        @Override
                        public void onPayloadReady(VirtualMachine vm) {
                            long start = System.nanoTime();
                            try {
                                vm.stop();
                            } catch (Exception e) {
                                Log.e(TAG, "Error in vm.stop():" + e);
                                throw new RuntimeException(e);
                            }
                            vmKillTime.add((double) (System.nanoTime() - start) / NANO_TO_MICRO);
                            super.onPayloadReady(vm);
                        }
                    };
            listener.runToFinish(TAG, vm);
        }
        reportMetrics(vmKillTime, "vm_kill_time", "microsecond");
    }
}
