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

package android.avf.test;

import static com.android.tradefed.device.TestDevice.MicrodroidBuilder;
import static com.android.tradefed.testtype.DeviceJUnit4ClassRunner.TestMetrics;

import static com.google.common.truth.Truth.assertWithMessage;
import static com.google.common.truth.TruthJUnit.assume;

import static org.junit.Assert.assertNotNull;
import static org.junit.Assume.assumeFalse;
import static org.junit.Assume.assumeTrue;

import android.platform.test.annotations.RootPermissionTest;

import com.android.microdroid.test.common.MetricsProcessor;
import com.android.microdroid.test.host.CommandRunner;
import com.android.microdroid.test.host.KvmHypTracer;
import com.android.microdroid.test.host.MicrodroidHostTestCaseBase;
import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.device.TestDevice;
import com.android.tradefed.log.LogUtil.CLog;
import com.android.tradefed.testtype.DeviceJUnit4ClassRunner;
import com.android.tradefed.util.CommandResult;
import com.android.tradefed.util.SimpleStats;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.runner.RunWith;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

@RootPermissionTest
@RunWith(DeviceJUnit4ClassRunner.class)
public final class AVFHostTestCase extends MicrodroidHostTestCaseBase {

    private static final String COMPOSD_CMD_BIN = "/apex/com.android.compos/bin/composd_cmd";

    // Files that define the "test" instance of CompOS
    private static final String COMPOS_TEST_ROOT = "/data/misc/apexdata/com.android.compos/test/";

    private static final String BOOTLOADER_TIME_PROP_NAME = "ro.boot.boottime";
    private static final String BOOTLOADER_PREFIX = "bootloader-";
    private static final String BOOTLOADER_TIME = "bootloader_time";
    private static final String BOOTLOADER_PHASE_SW = "SW";

    /** Boot time test related variables */
    private static final int REINSTALL_APEX_RETRY_INTERVAL_MS = 5 * 1000;

    private static final int REINSTALL_APEX_TIMEOUT_SEC = 15;
    private static final int COMPILE_STAGED_APEX_RETRY_INTERVAL_MS = 10 * 1000;
    private static final int COMPILE_STAGED_APEX_TIMEOUT_SEC = 540;
    private static final int BOOT_COMPLETE_TIMEOUT_MS = 10 * 60 * 1000;
    private static final int ROUND_COUNT = 5;
    private static final int ROUND_IGNORE_STARTUP_TIME = 3;
    private static final String APK_NAME = "MicrodroidTestApp.apk";
    private static final String PACKAGE_NAME = "com.android.microdroid.test";

    private MetricsProcessor mMetricsProcessor;
    @Rule public TestMetrics mMetrics = new TestMetrics();

    private boolean mNeedTearDown = false;

    @Before
    public void setUp() throws Exception {
        mNeedTearDown = false;

        assumeDeviceIsCapable(getDevice());
        mNeedTearDown = true;

        getDevice().installPackage(findTestFile(APK_NAME), /* reinstall */ false);

        mMetricsProcessor = new MetricsProcessor(getMetricPrefix() + "hostside/");
    }

    @After
    public void tearDown() throws Exception {
        if (!mNeedTearDown) {
            // If we skipped setUp, we don't need to undo it, and that avoids potential exceptions
            // incompatible hardware. (Note that tests can change what assumeDeviceIsCapable()
            // sees, so we can't rely on that - b/268688303.)
            return;
        }

        CommandRunner android = new CommandRunner(getDevice());

        // Clear up any CompOS instance files we created.
        android.tryRun("rm", "-rf", COMPOS_TEST_ROOT);
    }

    @Test
    public void testBootWithCompOS() throws Exception {
        composTestHelper(true);
    }

    @Test
    public void testBootWithoutCompOS() throws Exception {
        composTestHelper(false);
    }

    @Test
    public void testNoLongHypSections() throws Exception {
        String[] hypEvents = {"hyp_enter", "hyp_exit"};

        assumeTrue(
                "Skip without hypervisor tracing",
                KvmHypTracer.isSupported(getDevice(), hypEvents));

        KvmHypTracer tracer = new KvmHypTracer(getDevice(), hypEvents);
        String result = tracer.run(COMPOSD_CMD_BIN + " test-compile");
        assertWithMessage("Failed to test compilation VM.")
                .that(result)
                .ignoringCase()
                .contains("all ok");

        SimpleStats stats = tracer.getDurationStats();
        reportMetric(stats.getData(), "hyp_sections", "s");
        CLog.i("Hypervisor traces parsed successfully.");
    }

    @Test
    public void testPsciMemProtect() throws Exception {
        String[] hypEvents = {"psci_mem_protect"};

        assumeTrue(
                "Skip without hypervisor tracing",
                KvmHypTracer.isSupported(getDevice(), hypEvents));
        KvmHypTracer tracer = new KvmHypTracer(getDevice(), hypEvents);

        /* We need to wait for crosvm to die so all the VM pages are reclaimed */
        String result = tracer.run(COMPOSD_CMD_BIN + " test-compile && killall -w crosvm || true");
        assertWithMessage("Failed to test compilation VM.")
                .that(result)
                .ignoringCase()
                .contains("all ok");

        List<Integer> values = tracer.getPsciMemProtect();

        assertWithMessage("PSCI MEM_PROTECT events not recorded")
                .that(values.size())
                .isGreaterThan(2);

        assertWithMessage("PSCI MEM_PROTECT counter not starting from 0")
                .that(values.get(0))
                .isEqualTo(0);

        assertWithMessage("PSCI MEM_PROTECT counter not ending with 0")
                .that(values.get(values.size() - 1))
                .isEqualTo(0);

        assertWithMessage("PSCI MEM_PROTECT counter didn't increment")
                .that(Collections.max(values))
                .isGreaterThan(0);
    }

    @Test
    public void testCameraAppStartupTime() throws Exception {
        String[] launchIntentPackages = {
            "com.android.camera2",
            "com.google.android.GoogleCamera/com.android.camera.CameraLauncher"
        };
        String launchIntentPackage = findSupportedPackage(launchIntentPackages);
        assume().withMessage("No supported camera package").that(launchIntentPackage).isNotNull();
        appStartupHelper(launchIntentPackage);
    }

    @Test
    public void testSettingsAppStartupTime() throws Exception {
        String[] launchIntentPackages = {"com.android.settings"};
        String launchIntentPackage = findSupportedPackage(launchIntentPackages);
        assume().withMessage("No supported settings package").that(launchIntentPackage).isNotNull();
        appStartupHelper(launchIntentPackage);
    }

    private void appStartupHelper(String launchIntentPackage) throws Exception {
        assumeTrue(
                "Skip on non-protected VMs",
                ((TestDevice) getDevice()).supportsMicrodroid(/* protectedVm= */ true));

        StartupTimeMetricCollection mCollection =
                new StartupTimeMetricCollection(getPackageName(launchIntentPackage), ROUND_COUNT);
        getAppStartupTime(launchIntentPackage, mCollection);

        reportMetric(
                mCollection.mAppBeforeVmRunTotalTime,
                "app_startup/" + mCollection.getPkgName() + "/total_time/before_vm",
                "ms");
        reportMetric(
                mCollection.mAppBeforeVmRunWaitTime,
                "app_startup/" + mCollection.getPkgName() + "/wait_time/before_vm",
                "ms");
        reportMetric(
                mCollection.mAppDuringVmRunTotalTime,
                "app_startup/" + mCollection.getPkgName() + "/total_time/during_vm",
                "ms");
        reportMetric(
                mCollection.mAppDuringVmRunWaitTime,
                "app_startup/" + mCollection.getPkgName() + "/wait_time/during_vm",
                "ms");
        reportMetric(
                mCollection.mAppAfterVmRunTotalTime,
                "app_startup/" + mCollection.getPkgName() + "/total_time/after_vm",
                "ms");
        reportMetric(
                mCollection.mAppAfterVmRunWaitTime,
                "app_startup/" + mCollection.getPkgName() + "/wait_time/after_vm",
                "ms");
    }

    private String getPackageName(String launchIntentPackage) {
        String appPkg = launchIntentPackage;

        // Does the appPkgName contain the intent ?
        if (launchIntentPackage != null && launchIntentPackage.contains("/")) {
            appPkg = launchIntentPackage.split("/")[0];
        }
        return appPkg;
    }

    private String findSupportedPackage(String[] pkgNameList) throws Exception {
        CommandRunner android = new CommandRunner(getDevice());

        for (String pkgName : pkgNameList) {
            String appPkg = getPackageName(pkgName);
            String hasPackage =
                    android.run(
                            "pm list package | grep -w " + appPkg + " 1> /dev/null" + "; echo $?");
            assertNotNull(hasPackage);

            if (hasPackage.equals("0")) {
                return pkgName;
            }
        }
        return null;
    }

    private AmStartupTimeCmdParser getColdRunStartupTimes(CommandRunner android, String pkgName)
            throws DeviceNotAvailableException, InterruptedException {
        unlockScreen(android);
        // Ensure we are killing the app to get the cold app startup time
        android.run("am force-stop " + pkgName);
        android.run("echo 3 > /proc/sys/vm/drop_caches");
        String vmStartAppLog = android.run("am", "start -W -S " + pkgName);
        assertNotNull(vmStartAppLog);
        assumeFalse(vmStartAppLog.isEmpty());
        return new AmStartupTimeCmdParser(vmStartAppLog);
    }

    // Returns an array of two elements containing the delta between the initial app startup time
    // and the time measured after running the VM.
    private void getAppStartupTime(String pkgName, StartupTimeMetricCollection metricColector)
            throws Exception {
        TestDevice device = (TestDevice) getDevice();

        // 1. Reboot the device to run the test without stage2 fragmentation
        getDevice().rebootUntilOnline();
        waitForBootCompleted();

        // 2. Start the app and ignore first runs to warm up caches
        CommandRunner android = new CommandRunner(getDevice());
        for (int i = 0; i < ROUND_IGNORE_STARTUP_TIME; i++) {
            getColdRunStartupTimes(android, pkgName);
        }

        // 3. Run the app before the VM run and collect app startup time statistics
        for (int i = 0; i < ROUND_COUNT; i++) {
            AmStartupTimeCmdParser beforeVmStartApp = getColdRunStartupTimes(android, pkgName);
            metricColector.addStartupTimeMetricBeforeVmRun(beforeVmStartApp);
        }

        // Clear up any test dir
        android.tryRun("rm", "-rf", MicrodroidHostTestCaseBase.TEST_ROOT);

        // Donate 80% of the available device memory to the VM
        final String configPath = "assets/vm_config.json";
        final int vm_mem_mb = getFreeMemoryInfoMb(android) * 80 / 100;
        ITestDevice microdroidDevice =
                MicrodroidBuilder.fromDevicePath(getPathForPackage(PACKAGE_NAME), configPath)
                        .debugLevel("full")
                        .memoryMib(vm_mem_mb)
                        .cpuTopology("match_host")
                        .build(device);
        try {
            microdroidDevice.waitForBootComplete(30000);
            microdroidDevice.enableAdbRoot();

            CommandRunner microdroid = new CommandRunner(microdroidDevice);

            microdroid.run("mkdir -p /mnt/ramdisk && chmod 777 /mnt/ramdisk");
            microdroid.run("mount -t tmpfs -o size=32G tmpfs /mnt/ramdisk");

            // Allocate memory for the VM until it fails and make sure that we touch
            // the allocated memory in the guest to be able to create stage2 fragmentation.
            try {
                microdroid.tryRun(
                        String.format(
                                "cd /mnt/ramdisk && truncate -s %dM sprayMemory"
                                        + " && dd if=/dev/zero of=sprayMemory bs=1MB count=%d",
                                vm_mem_mb, vm_mem_mb));
            } catch (Exception expected) {
            }

            // Run the app during the VM run and collect cold startup time.
            for (int i = 0; i < ROUND_COUNT; i++) {
                AmStartupTimeCmdParser duringVmStartApp = getColdRunStartupTimes(android, pkgName);
                metricColector.addStartupTimeMetricDuringVmRun(duringVmStartApp);
            }
        } finally {
            device.shutdownMicrodroid(microdroidDevice);
        }

        // Run the app after the VM run and collect cold startup time.
        for (int i = 0; i < ROUND_COUNT; i++) {
            AmStartupTimeCmdParser afterVmStartApp = getColdRunStartupTimes(android, pkgName);
            metricColector.addStartupTimerMetricAfterVmRun(afterVmStartApp);
        }
    }

    static class AmStartupTimeCmdParser {
        private int mTotalTime;
        private int mWaitTime;

        AmStartupTimeCmdParser(String startAppLog) {
            String[] lines = startAppLog.split("[\r\n]+");
            mTotalTime = mWaitTime = 0;

            for (String line : lines) {
                if (line.contains("TotalTime:")) {
                    mTotalTime = Integer.parseInt(line.replaceAll("\\D+", ""));
                }
                if (line.contains("WaitTime:")) {
                    mWaitTime = Integer.parseInt(line.replaceAll("\\D+", ""));
                }
            }
        }
    }

    static class StartupTimeMetricCollection {
        List<Double> mAppBeforeVmRunTotalTime;
        List<Double> mAppBeforeVmRunWaitTime;

        List<Double> mAppDuringVmRunTotalTime;
        List<Double> mAppDuringVmRunWaitTime;

        List<Double> mAppAfterVmRunTotalTime;
        List<Double> mAppAfterVmRunWaitTime;

        private final String mPkgName;

        StartupTimeMetricCollection(String pkgName, int size) {
            mAppBeforeVmRunTotalTime = new ArrayList<>(size);
            mAppBeforeVmRunWaitTime = new ArrayList<>(size);

            mAppDuringVmRunTotalTime = new ArrayList<>(size);
            mAppDuringVmRunWaitTime = new ArrayList<>(size);

            mAppAfterVmRunTotalTime = new ArrayList<>(size);
            mAppAfterVmRunWaitTime = new ArrayList<>(size);
            mPkgName = pkgName;
        }

        public void addStartupTimeMetricBeforeVmRun(AmStartupTimeCmdParser m) {
            mAppBeforeVmRunTotalTime.add((double) m.mTotalTime);
            mAppBeforeVmRunWaitTime.add((double) m.mWaitTime);
        }

        public void addStartupTimeMetricDuringVmRun(AmStartupTimeCmdParser m) {
            mAppDuringVmRunTotalTime.add((double) m.mTotalTime);
            mAppDuringVmRunWaitTime.add((double) m.mWaitTime);
        }

        public void addStartupTimerMetricAfterVmRun(AmStartupTimeCmdParser m) {
            mAppAfterVmRunTotalTime.add((double) m.mTotalTime);
            mAppAfterVmRunWaitTime.add((double) m.mWaitTime);
        }

        public String getPkgName() {
            return this.mPkgName;
        }
    }

    private int getFreeMemoryInfoMb(CommandRunner android)
            throws DeviceNotAvailableException, IllegalArgumentException {
        int freeMemory = 0;
        String content = android.runForResult("cat /proc/meminfo").getStdout().trim();
        String[] lines = content.split("[\r\n]+");

        for (String line : lines) {
            if (line.contains("MemFree:")) {
                freeMemory = Integer.parseInt(line.replaceAll("\\D+", "")) / 1024;
                return freeMemory;
            }
        }

        throw new IllegalArgumentException();
    }

    private void unlockScreen(CommandRunner android)
            throws DeviceNotAvailableException, InterruptedException {
        android.run("input keyevent", "KEYCODE_WAKEUP");
        Thread.sleep(500);
        final String ret =
                android.runForResult("dumpsys nfc | grep 'mScreenState='").getStdout().trim();
        if (ret != null && ret.contains("ON_LOCKED")) {
            android.run("input keyevent", "KEYCODE_MENU");
        }
    }

    private void updateBootloaderTimeInfo(Map<String, List<Double>> bootloaderTime)
            throws Exception {

        String bootLoaderVal = getDevice().getProperty(BOOTLOADER_TIME_PROP_NAME);
        // Sample Output : 1BLL:89,1BLE:590,2BLL:0,2BLE:1344,SW:6734,KL:1193
        if (bootLoaderVal != null) {
            String[] bootLoaderPhases = bootLoaderVal.split(",");
            double bootLoaderTotalTime = 0d;
            for (String bootLoaderPhase : bootLoaderPhases) {
                String[] bootKeyVal = bootLoaderPhase.split(":");
                String key = String.format("%s%s", BOOTLOADER_PREFIX, bootKeyVal[0]);

                bootloaderTime
                        .computeIfAbsent(key, k -> new ArrayList<>())
                        .add(Double.parseDouble(bootKeyVal[1]));
                // SW is the time spent on the warning screen. So ignore it in
                // final boot time calculation.
                if (BOOTLOADER_PHASE_SW.equalsIgnoreCase(bootKeyVal[0])) {
                    continue;
                }
                bootLoaderTotalTime += Double.parseDouble(bootKeyVal[1]);
            }
            bootloaderTime
                    .computeIfAbsent(BOOTLOADER_TIME, k -> new ArrayList<>())
                    .add(bootLoaderTotalTime);
        }
    }

    private Double getDmesgBootTime() throws Exception {

        CommandRunner android = new CommandRunner(getDevice());
        String result = android.run("dmesg");
        Pattern pattern = Pattern.compile("\\[(.*)].*sys.boot_completed=1.*");
        for (String line : result.split("[\r\n]+")) {
            Matcher matcher = pattern.matcher(line);
            if (matcher.find()) {
                return Double.valueOf(matcher.group(1));
            }
        }
        throw new IllegalArgumentException("Failed to get boot time info.");
    }

    private void composTestHelper(boolean isWithCompos) throws Exception {
        assumeFalse("Skip on CF; too slow", isCuttlefish());

        List<Double> bootDmesgTime = new ArrayList<>(ROUND_COUNT);

        for (int round = 0; round < ROUND_COUNT; ++round) {
            reInstallApex(REINSTALL_APEX_TIMEOUT_SEC);
            try {
                if (isWithCompos) {
                    compileStagedApex(COMPILE_STAGED_APEX_TIMEOUT_SEC);
                }
            } finally {
                // If compilation fails, we still have a staged APEX, and we need to reboot to
                // clean that up for further tests.
                getDevice().nonBlockingReboot();
                waitForBootCompleted();
            }

            double elapsedSec = getDmesgBootTime();
            bootDmesgTime.add(elapsedSec);
        }

        String suffix = "";
        if (isWithCompos) {
            suffix = "with_compos";
        } else {
            suffix = "without_compos";
        }

        reportMetric(bootDmesgTime, "dmesg_boot_time_" + suffix, "s");
    }

    private void reportMetric(List<Double> data, String name, String unit) {
        CLog.d("Report metric " + name + "(" + unit + ") : " + data.toString());
        Map<String, Double> stats = mMetricsProcessor.computeStats(data, name, unit);
        for (Map.Entry<String, Double> entry : stats.entrySet()) {
            CLog.d("Add test metrics " + entry.getKey() + " : " + entry.getValue().toString());
            mMetrics.addTestMetric(entry.getKey(), entry.getValue().toString());
        }
    }

    private void waitForBootCompleted() throws Exception {
        getDevice().waitForDeviceOnline(BOOT_COMPLETE_TIMEOUT_MS);
        getDevice().waitForBootComplete(BOOT_COMPLETE_TIMEOUT_MS);
        getDevice().enableAdbRoot();
    }

    private void compileStagedApex(int timeoutSec) throws Exception {

        long timeStart = System.currentTimeMillis();
        long timeEnd = timeStart + timeoutSec * 1000L;

        while (true) {

            try {
                CommandRunner android = new CommandRunner(getDevice());

                String result =
                        android.runWithTimeout(
                                3 * 60 * 1000, COMPOSD_CMD_BIN + " staged-apex-compile");
                assertWithMessage("Failed to compile staged APEX. Reason: " + result)
                        .that(result)
                        .ignoringCase()
                        .contains("all ok");

                CLog.i("Success to compile staged APEX. Result: " + result);

                break;
            } catch (AssertionError e) {
                CLog.i("Gets AssertionError when compile staged APEX. Detail: " + e);
            }

            if (System.currentTimeMillis() > timeEnd) {
                CLog.e("Try to compile staged APEX several times but all fail.");
                throw new AssertionError("Failed to compile staged APEX.");
            }

            Thread.sleep(COMPILE_STAGED_APEX_RETRY_INTERVAL_MS);
        }
    }

    private void reInstallApex(int timeoutSec) throws Exception {

        long timeStart = System.currentTimeMillis();
        long timeEnd = timeStart + timeoutSec * 1000L;

        while (true) {

            try {
                CommandRunner android = new CommandRunner(getDevice());

                String packagesOutput = android.run("pm list packages -f --apex-only");

                Pattern p =
                        Pattern.compile(
                                "package:(.*)=(com(?:\\.google)?\\.android\\.art)$",
                                Pattern.MULTILINE);
                Matcher m = p.matcher(packagesOutput);
                assertWithMessage("ART module not found. Packages are:\n" + packagesOutput)
                        .that(m.find())
                        .isTrue();

                String artApexPath = m.group(1);

                CommandResult result = android.runForResult("pm install --apex " + artApexPath);
                assertWithMessage("Failed to install APEX. Reason: " + result)
                        .that(result.getExitCode())
                        .isEqualTo(0);

                CLog.i("Success to install APEX. Result: " + result);

                break;
            } catch (AssertionError e) {
                CLog.i("Gets AssertionError when reinstall art APEX. Detail: " + e);
            }

            if (System.currentTimeMillis() > timeEnd) {
                CLog.e("Try to reinstall art APEX several times but all fail.");
                throw new AssertionError("Failed to reinstall art APEX.");
            }

            Thread.sleep(REINSTALL_APEX_RETRY_INTERVAL_MS);
        }
    }
}
