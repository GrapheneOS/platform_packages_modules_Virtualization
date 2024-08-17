/*
 * Copyright (C) 2021 The Android Open Source Project
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

import static com.android.tradefed.testtype.DeviceJUnit4ClassRunner.TestLogData;

import static com.google.common.truth.Truth.assertWithMessage;

import static org.junit.Assume.assumeFalse;
import static org.junit.Assume.assumeTrue;

import com.android.compatibility.common.tradefed.build.CompatibilityBuildHelper;
import com.android.microdroid.test.common.DeviceProperties;
import com.android.microdroid.test.common.MetricsProcessor;
import com.android.tradefed.build.IBuildInfo;
import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.device.TestDevice;
import com.android.tradefed.testtype.junit4.BaseHostJUnit4Test;
import com.android.tradefed.util.CommandResult;
import com.android.tradefed.util.CommandStatus;
import com.android.tradefed.util.FileUtil;
import com.android.tradefed.util.RunUtil;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.stream.Collectors;

public abstract class MicrodroidHostTestCaseBase extends BaseHostJUnit4Test {
    protected static final String TEST_ROOT = "/data/local/tmp/virt/";
    protected static final String TRADEFED_TEST_ROOT = "/data/local/tmp/virt/tradefed/";
    protected static final String LOG_PATH = TEST_ROOT + "log.txt";
    protected static final String CONSOLE_PATH = TEST_ROOT + "console.txt";
    protected static final String TRADEFED_CONSOLE_PATH = TRADEFED_TEST_ROOT + "console.txt";
    protected static final String TRADEFED_LOG_PATH = TRADEFED_TEST_ROOT + "log.txt";
    private static final int TEST_VM_ADB_PORT = 8000;
    private static final String MICRODROID_SERIAL = "localhost:" + TEST_VM_ADB_PORT;
    private static final String INSTANCE_IMG = "instance.img";
    protected static final String VIRT_APEX = "/apex/com.android.virt/";
    protected static final String SECRETKEEPER_AIDL =
            "android.hardware.security.secretkeeper.ISecretkeeper/default";

    private static final long MICRODROID_ADB_CONNECT_TIMEOUT_MINUTES = 5;
    protected static final long MICRODROID_COMMAND_TIMEOUT_MILLIS = 30000;
    private static final long MICRODROID_COMMAND_RETRY_INTERVAL_MILLIS = 500;
    protected static final int MICRODROID_ADB_CONNECT_MAX_ATTEMPTS =
            (int)
                    (MICRODROID_ADB_CONNECT_TIMEOUT_MINUTES
                            * 60
                            * 1000
                            / MICRODROID_COMMAND_RETRY_INTERVAL_MILLIS);

    protected static final Set<String> SUPPORTED_GKI_VERSIONS =
            Collections.unmodifiableSet(new HashSet(Arrays.asList("android15-6.6")));

    /* Keep this sync with AssignableDevice.aidl */
    public static final class AssignableDevice {
        public final String node;
        public final String dtbo_label;

        public AssignableDevice(String node, String dtbo_label) {
            this.node = node;
            this.dtbo_label = dtbo_label;
        }
    }

    public static void prepareVirtualizationTestSetup(ITestDevice androidDevice)
            throws DeviceNotAvailableException {
        CommandRunner android = new CommandRunner(androidDevice);

        // kill stale crosvm processes
        android.tryRun("killall", "crosvm");

        // disconnect from microdroid
        tryRunOnHost("adb", "disconnect", MICRODROID_SERIAL);

        // remove any leftover files under test root
        android.tryRun("rm", "-rf", TEST_ROOT + "*");

        android.tryRun("mkdir " + TEST_ROOT);
    }

    public static void cleanUpVirtualizationTestSetup(ITestDevice androidDevice)
            throws DeviceNotAvailableException {
        CommandRunner android = new CommandRunner(androidDevice);

        // disconnect from microdroid
        tryRunOnHost("adb", "disconnect", MICRODROID_SERIAL);

        // kill stale VMs and directories
        android.tryRun("killall", "crosvm");
        android.tryRun("stop", "virtualizationservice");
        android.tryRun("rm", "-rf", "/data/misc/virtualizationservice/*");
    }

    public boolean isUserBuild() {
        return DeviceProperties.create(getDevice()::getProperty).isUserBuild();
    }

    protected boolean isCuttlefish() {
        return DeviceProperties.create(getDevice()::getProperty).isCuttlefish();
    }

    protected boolean isHwasan() {
        return DeviceProperties.create(getDevice()::getProperty).isHwasan();
    }

    protected String getMetricPrefix() {
        return MetricsProcessor.getMetricPrefix(
                DeviceProperties.create(getDevice()::getProperty).getMetricsTag());
    }

    public static void assumeDeviceIsCapable(ITestDevice androidDevice) throws Exception {
        assumeTrue("Need an actual TestDevice", androidDevice instanceof TestDevice);
        TestDevice testDevice = (TestDevice) androidDevice;
        assumeTrue(
                "Requires VM support",
                testDevice.hasFeature("android.software.virtualization_framework"));
        assumeTrue("Requires VM support", testDevice.supportsMicrodroid());

        CommandRunner android = new CommandRunner(androidDevice);
        long vendorApiLevel = androidDevice.getIntProperty("ro.board.api_level", 0);
        boolean isGsi =
                android.runForResult("[ -e /system/system_ext/etc/init/init.gsi.rc ]").getStatus()
                        == CommandStatus.SUCCESS;
        assumeFalse(
                "GSI with vendor API level < 202404 may not support AVF",
                isGsi && vendorApiLevel < 202404);
    }

    public static void archiveLogThenDelete(
            TestLogData logs, ITestDevice device, String remotePath, String localName)
            throws DeviceNotAvailableException {
        LogArchiver.archiveLogThenDelete(logs, device, remotePath, localName);
    }

    public static void setPropertyOrThrow(ITestDevice device, String propertyName, String value)
            throws DeviceNotAvailableException {
        if (!device.setProperty(propertyName, value)) {
            throw new RuntimeException("Failed to set sysprop " + propertyName + " to " + value);
        }
    }

    // Run an arbitrary command in the host side and returns the result.
    // Note failure is not an error.
    public static String tryRunOnHost(String... cmd) {
        final long timeout = 10000;
        CommandResult result = RunUtil.getDefault().runTimedCmd(timeout, cmd);
        return result.getStdout().trim();
    }

    private static String join(String... strs) {
        return String.join(" ", Arrays.asList(strs));
    }

    public File findTestFile(String name) {
        String moduleName = getInvocationContext().getConfigurationDescriptor().getModuleName();
        IBuildInfo buildInfo = getBuild();
        CompatibilityBuildHelper helper = new CompatibilityBuildHelper(buildInfo);

        // We're not using helper.getTestFile here because it sometimes picks a file
        // from a different module, which may be old and/or wrong. See b/328779049.
        try {
            File testsDir = helper.getTestsDir().getAbsoluteFile();

            for (File subDir : FileUtil.findDirsUnder(testsDir, testsDir.getParentFile())) {
                if (!subDir.getName().equals(moduleName)) {
                    continue;
                }
                File testFile = FileUtil.findFile(subDir, name);
                if (testFile != null) {
                    return testFile;
                }
            }
        } catch (IOException e) {
            throw new AssertionError(
                    "Failed to find test file " + name + " for module " + moduleName, e);
        }
        throw new AssertionError("Failed to find test file " + name + " for module " + moduleName);
    }

    public String getPathForPackage(String packageName) throws DeviceNotAvailableException {
        return getPathForPackage(getDevice(), packageName);
    }

    // Get the path to the installed apk. Note that
    // getDevice().getAppPackageInfo(...).getCodePath() doesn't work due to the incorrect
    // parsing of the "=" character. (b/190975227). So we use the `pm path` command directly.
    private static String getPathForPackage(ITestDevice device, String packageName)
            throws DeviceNotAvailableException {
        CommandRunner android = new CommandRunner(device);
        String pathLine = android.run("pm", "path", packageName);
        assertWithMessage("Package " + packageName + " not found")
                .that(pathLine)
                .startsWith("package:");
        return pathLine.substring("package:".length());
    }

    public String parseFieldFromVmInfo(String header) throws Exception {
        CommandRunner android = new CommandRunner(getDevice());
        String result = android.run("/apex/com.android.virt/bin/vm", "info");
        for (String line : result.split("\n")) {
            if (!line.startsWith(header)) continue;

            return line.substring(header.length());
        }
        return "";
    }

    public List<String> parseStringArrayFieldsFromVmInfo(String header) throws Exception {
        String field = parseFieldFromVmInfo(header);

        List<String> ret = new ArrayList<>();
        if (!field.isEmpty()) {
            JSONArray jsonArray = new JSONArray(field);
            for (int i = 0; i < jsonArray.length(); i++) {
                ret.add(jsonArray.getString(i));
            }
        }
        return ret;
    }

    public boolean isFeatureEnabled(String feature) throws Exception {
        CommandRunner android = new CommandRunner(getDevice());
        String result = android.run(VIRT_APEX + "bin/vm", "check-feature-enabled", feature);
        return result.contains("enabled");
    }

    public List<AssignableDevice> getAssignableDevices() throws Exception {
        String field = parseFieldFromVmInfo("Assignable devices: ");

        List<AssignableDevice> ret = new ArrayList<>();
        if (!field.isEmpty()) {
            JSONArray jsonArray = new JSONArray(field);
            for (int i = 0; i < jsonArray.length(); i++) {
                JSONObject jsonObject = jsonArray.getJSONObject(i);
                ret.add(
                        new AssignableDevice(
                                jsonObject.getString("node"), jsonObject.getString("dtbo_label")));
            }
        }
        return ret;
    }

    public boolean isUpdatableVmSupported() throws DeviceNotAvailableException {
        // Updatable VMs are possible iff device supports Secretkeeper.
        CommandRunner android = new CommandRunner(getDevice());
        CommandResult result = android.runForResult("service check", SECRETKEEPER_AIDL);
        assertWithMessage("Failed to run service check. Result= " + result)
                .that(result.getStatus() == CommandStatus.SUCCESS && result.getExitCode() == 0)
                .isTrue();
        boolean is_sk_supported = !result.getStdout().trim().contains("not found");
        return is_sk_supported;
    }

    public List<String> getSupportedOSList() throws Exception {
        return parseStringArrayFieldsFromVmInfo("Available OS list: ");
    }

    public List<String> getSupportedGKIVersions() throws Exception {
        return getSupportedOSList().stream()
                .filter(os -> os.startsWith("microdroid_gki-"))
                .map(os -> os.replaceFirst("^microdroid_gki-", ""))
                .collect(Collectors.toList());
    }

    protected boolean isPkvmHypervisor() throws DeviceNotAvailableException {
        return "kvm.arm-protected".equals(getDevice().getProperty("ro.boot.hypervisor.version"));
    }
}
