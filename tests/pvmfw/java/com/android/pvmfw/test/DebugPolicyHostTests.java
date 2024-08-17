/*
 * Copyright 2023 The Android Open Source Project
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

package com.android.pvmfw.test;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.Truth.assertWithMessage;

import static org.junit.Assert.assertThrows;

import androidx.annotation.NonNull;
import androidx.annotation.Nullable;

import com.android.microdroid.test.host.CommandRunner;
import com.android.pvmfw.test.host.Pvmfw;
import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.DeviceRuntimeException;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.testtype.DeviceJUnit4ClassRunner;
import com.android.tradefed.util.CommandResult;
import com.android.tradefed.util.CommandStatus;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;

import java.io.File;
import java.util.Collections;
import java.util.Map;
import java.util.concurrent.TimeUnit;

/** Tests debug policy */
@RunWith(DeviceJUnit4ClassRunner.class)
public class DebugPolicyHostTests extends CustomPvmfwHostTestCaseBase {
    @NonNull private static final String CUSTOM_DEBUG_POLICY_FILE_NAME = "debug_policy.dtb";

    @NonNull
    private static final String CUSTOM_DEBUG_POLICY_PATH =
            TRADEFED_TEST_ROOT + CUSTOM_DEBUG_POLICY_FILE_NAME;

    @NonNull
    private static final String CUSTOM_DEBUG_POLICY_PATH_PROP =
            "hypervisor.virtualizationmanager.debug_policy.path";

    @NonNull
    private static final String AVF_DEBUG_POLICY_ADB_DT_PROP_PATH = "/avf/guest/microdroid/adb";

    @NonNull private static final String MICRODROID_CMDLINE_PATH = "/proc/cmdline";
    @NonNull private static final String MICRODROID_DT_ROOT_PATH = "/proc/device-tree";

    @NonNull
    private static final String MICRODROID_DT_BOOTARGS_PATH =
            MICRODROID_DT_ROOT_PATH + "/chosen/bootargs";

    @NonNull
    private static final String MICRODROID_DT_RAMDUMP_PATH =
            MICRODROID_DT_ROOT_PATH + "/avf/guest/common/ramdump";

    @NonNull private static final String HEX_STRING_ZERO = "00000000";
    @NonNull private static final String HEX_STRING_ONE = "00000001";

    @Nullable private File mCustomDebugPolicyFileOnHost;

    @Before
    public void setUp() throws Exception {
        super.setUp();

        // Prepare system properties for custom debug policy.
        setPropertyOrThrow(getDevice(), CUSTOM_DEBUG_POLICY_PATH_PROP, CUSTOM_DEBUG_POLICY_PATH);
    }

    @After
    public void shutdown() throws Exception {
        super.shutdown();

        // Cleanup for custom debug policies
        setPropertyOrThrow(getDevice(), CUSTOM_DEBUG_POLICY_PATH_PROP, "");
    }

    @Test
    public void testAdbInDebugPolicy_withDebugLevelNone_bootWithAdbConnection() throws Exception {
        prepareCustomDebugPolicy("avf_debug_policy_with_adb.dtbo");

        launchProtectedVmAndWaitForBootCompleted(MICRODROID_DEBUG_NONE);
    }

    @Test
    public void testNoAdbInDebugPolicy_withDebugLevelNone_boots() throws Exception {
        prepareCustomDebugPolicy("avf_debug_policy_without_adb.dtbo");

        // VM would boot, but cannot verify directly because of no adbd in the VM.
        CommandResult result = tryLaunchProtectedNonDebuggableVm();
        assertThat(result.getStatus()).isEqualTo(CommandStatus.TIMED_OUT);
        assertWithMessage("Microdroid should have booted")
                .that(result.getStderr())
                .contains("payload is ready");
    }

    @Test
    public void testNoAdbInDebugPolicy_withDebugLevelNone_noConnection() throws Exception {
        prepareCustomDebugPolicy("avf_debug_policy_without_adb.dtbo");

        assertThrows(
                "Microdroid shouldn't be recognized because of missing adb connection",
                DeviceRuntimeException.class,
                () ->
                        launchProtectedVmAndWaitForBootCompleted(
                                MICRODROID_DEBUG_NONE, BOOT_FAILURE_WAIT_TIME_MS));
    }

    @Test
    public void testNoAdbInDebugPolicy_withDebugLevelFull_bootWithAdbConnection() throws Exception {
        prepareCustomDebugPolicy("avf_debug_policy_without_adb.dtbo");

        launchProtectedVmAndWaitForBootCompleted(MICRODROID_DEBUG_FULL);
    }

    @Test
    public void testRamdumpInDebugPolicy_withDebugLevelNone_hasRamdumpArgs() throws Exception {
        prepareCustomDebugPolicy("avf_debug_policy_with_ramdump.dtbo");
        ITestDevice device = launchProtectedVmAndWaitForBootCompleted(MICRODROID_DEBUG_NONE);

        assertThat(readFileAsString(device, MICRODROID_CMDLINE_PATH)).contains("crashkernel=");
        assertThat(readFileAsString(device, MICRODROID_DT_BOOTARGS_PATH)).contains("crashkernel=");
        assertThat(readFileAsHexString(device, MICRODROID_DT_RAMDUMP_PATH))
                .isEqualTo(HEX_STRING_ONE);
    }

    @Test
    public void testNoRamdumpInDebugPolicy_withDebugLevelNone_noRamdumpArgs() throws Exception {
        prepareCustomDebugPolicy("avf_debug_policy_without_ramdump.dtbo");
        ITestDevice device = launchProtectedVmAndWaitForBootCompleted(MICRODROID_DEBUG_NONE);

        assertThat(readFileAsString(device, MICRODROID_CMDLINE_PATH))
                .doesNotContain("crashkernel=");
        assertThat(readFileAsString(device, MICRODROID_DT_BOOTARGS_PATH))
                .doesNotContain("crashkernel=");
        assertThat(readFileAsHexString(device, MICRODROID_DT_RAMDUMP_PATH))
                .isEqualTo(HEX_STRING_ZERO);
    }

    @Test
    public void testNoRamdumpInDebugPolicy_withDebugLevelFull_hasRamdumpArgs() throws Exception {
        prepareCustomDebugPolicy("avf_debug_policy_without_ramdump.dtbo");
        ITestDevice device = launchProtectedVmAndWaitForBootCompleted(MICRODROID_DEBUG_FULL);

        assertThat(readFileAsString(device, MICRODROID_CMDLINE_PATH)).contains("crashkernel=");
        assertThat(readFileAsString(device, MICRODROID_DT_BOOTARGS_PATH)).contains("crashkernel=");
        assertThat(readFileAsHexString(device, MICRODROID_DT_RAMDUMP_PATH))
                .isEqualTo(HEX_STRING_ZERO);
    }

    private boolean isDebugPolicyEnabled(@NonNull String dtPropertyPath)
            throws DeviceNotAvailableException {
        CommandRunner runner = new CommandRunner(getDevice());
        CommandResult result =
                runner.runForResult("xxd", "-p", "/proc/device-tree" + dtPropertyPath);
        if (result.getStatus() == CommandStatus.SUCCESS) {
            return HEX_STRING_ONE.equals(result.getStdout().trim());
        }
        return false;
    }

    @NonNull
    private String readFileAsString(@NonNull ITestDevice device, @NonNull String path)
            throws DeviceNotAvailableException {
        return new CommandRunner(device).run("cat", path);
    }

    @NonNull
    private String readFileAsHexString(@NonNull ITestDevice device, @NonNull String path)
            throws DeviceNotAvailableException {
        return new CommandRunner(device).run("xxd", "-p", path);
    }

    private void prepareCustomDebugPolicy(@NonNull String debugPolicyFileName) throws Exception {
        mCustomDebugPolicyFileOnHost =
                getTestInformation()
                        .getDependencyFile(debugPolicyFileName, /* targetFirst= */ false);

        Pvmfw.Builder builder =
                new Pvmfw.Builder(getPvmfwBinFile(), getBccFile())
                        .setDebugPolicyOverlay(mCustomDebugPolicyFileOnHost);
        if (isSecretKeeperSupported()) {
            builder.setVmReferenceDt(getVmReferenceDtFile());
        } else {
            builder.setVersion(1, 1);
        }
        Pvmfw pvmfw = builder.build();
        pvmfw.serialize(getCustomPvmfwFile());
    }

    private boolean hasConsoleOutput(@NonNull CommandResult result)
            throws DeviceNotAvailableException {
        return result.getStdout().contains("Run /init as init process");
    }

    private boolean hasMicrodroidLogcatOutput() throws DeviceNotAvailableException {
        CommandResult result =
                new CommandRunner(getDevice()).runForResult("test", "-s", MICRODROID_LOG_PATH);
        return result.getExitCode() == 0;
    }

    public ITestDevice launchProtectedVmAndWaitForBootCompleted(String debugLevel)
            throws DeviceNotAvailableException {
        return launchProtectedVmAndWaitForBootCompleted(debugLevel, BOOT_COMPLETE_TIMEOUT_MS);
    }

    public ITestDevice launchProtectedVmAndWaitForBootCompleted(
            String debugLevel, long adbTimeoutMs) throws DeviceNotAvailableException {
        Map<String, File> bootFiles =
                Collections.singletonMap(
                        CUSTOM_DEBUG_POLICY_FILE_NAME, mCustomDebugPolicyFileOnHost);

        return launchProtectedVmAndWaitForBootCompleted(debugLevel, adbTimeoutMs, bootFiles);
    }

    // Try to launch protected non-debuggable VM for a while and quit.
    // Non-debuggable VM might not enable adb, so there's no ITestDevice instance of it.
    private CommandResult tryLaunchProtectedNonDebuggableVm() throws Exception {
        // Can't use MicrodroidBuilder because it expects adb connection
        // but non-debuggable VM may not enable adb.
        CommandRunner runner = new CommandRunner(getDevice());
        runner.run("mkdir", "-p", TEST_ROOT);
        getDevice().pushFile(getCustomPvmfwFile(), CUSTOM_PVMFW_IMG_PATH);
        getDevice().pushFile(mCustomDebugPolicyFileOnHost, CUSTOM_DEBUG_POLICY_PATH);

        // This will fail because app wouldn't finish itself.
        // But let's run the app once and get logs.
        String command =
                String.join(
                        " ",
                        VIRT_APEX + "bin/vm",
                        "run-app",
                        "--log",
                        MICRODROID_LOG_PATH,
                        "--protected",
                        getPathForPackage(PACKAGE_NAME),
                        TEST_ROOT + "idsig",
                        TEST_ROOT + "instance.img",
                        "--config-path",
                        MICRODROID_CONFIG_PATH);
        if (isFeatureEnabled("com.android.kvm.LLPVM_CHANGES")) {
            command = String.join(" ", command, "--instance-id-file", TEST_ROOT + "instance_id");
        }
        return getDevice()
                .executeShellV2Command(
                        command,
                        CONSOLE_OUTPUT_WAIT_MS,
                        TimeUnit.MILLISECONDS,
                        /* retryAttempts= */ 0);
    }
}
