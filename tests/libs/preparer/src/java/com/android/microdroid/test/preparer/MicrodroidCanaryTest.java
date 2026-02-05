/*
 * Copyright 2025 The Android Open Source Project
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

package com.android.microdroid.test.preparer;

import com.android.tradefed.config.OptionClass;
import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.DeviceProperties;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.device.TestDevice;
import com.android.tradefed.device.TestDevice.MicrodroidBuilder;
import com.android.tradefed.invoker.TestInformation;
import com.android.tradefed.log.LogUtil.CLog;
import com.android.tradefed.targetprep.BaseTargetPreparer;
import com.android.tradefed.util.CommandResult;
import com.android.tradefed.util.CommandStatus;

import java.util.Arrays;
import java.util.List;

/**
 * The minimalist canary test that helps to skip test as a whole.
 *
 * <p>This is intentionally a preparer (not a ModuleController), so failure can be still reported as
 * test failure.
 */
@OptionClass(alias = "microdroid-canary")
public class MicrodroidCanaryTest extends BaseTargetPreparer {
    // These are bundled inside the virt apex, so one should be exist if AVF is supported.
    private static final List<String> DEFAULT_PAYLOAD_APKS =
            Arrays.asList(
                    "com.android.microdroid.empty_payload",
                    "com.google.android.microdroid.empty_payload");
    private static final String DEFAULT_PAYLOAD_BINARY_NAME = "MicrodroidEmptyPayloadJniLib.so";

    private static final long TIMEOUT_MS = 60_000L;
    private static final long LONG_TIMEOUT_MS = 300_000L;

    private String getPathForPayloadPackage(TestDevice device) throws DeviceNotAvailableException {
        for (String apk : DEFAULT_PAYLOAD_APKS) {
            String cmd = "pm path " + apk;
            CommandResult result = device.executeShellV2Command(cmd);
            if (result.getStatus() != CommandStatus.SUCCESS) {
                continue;
            }

            String stdout = result.getStdout().trim();
            if (!stdout.startsWith("package:")) {
                continue;
            }

            return stdout.substring("package:".length());
        }

        return null;
    }

    private String chooseOs(TestDevice device) throws DeviceNotAvailableException {
        int apiLevel = device.getApiLevel();
        if (device.getApiLevel() < 36) {
            // No `--os` support on older device.
            return "";
        } else {
            // 16K is supported since API level 35, so check if the device is capable.
            CommandResult result = device.executeShellV2Command("getconf PAGE_SIZE");
            String stdout = result.getStdout().trim();
            if ("16384".equals(stdout)) { // 16K
                return "microdroid_16k";
            }
            return "microdroid";
        }
    }

    private boolean isVirtualDevice(TestDevice device) throws DeviceNotAvailableException {
        String vendorDeviceName = device.getProperty(DeviceProperties.VARIANT);
        return vendorDeviceName != null
                && (vendorDeviceName.startsWith("vsoc_") || vendorDeviceName.startsWith("emu64"));
    }

    public void ensureMicrodroidBoot(
            TestDevice device, String apkPath, String os, boolean protectedVm)
            throws DeviceNotAvailableException {
        try {
            if (!device.supportsMicrodroid(protectedVm)) {
                CLog.d(
                        "Unsupported device. Skipping microdroid canary, protectedVm=%s",
                        protectedVm);
                return;
            }
        } catch (Exception e) {
            // Workaround to reduce exception
            throw new DeviceNotAvailableException(
                    "Failed to check whether Microdroid is supported");
        }

        long timeoutMs = isVirtualDevice(device) ? LONG_TIMEOUT_MS : TIMEOUT_MS;

        ITestDevice microdroid = null;
        final String errorMessage =
                "Failed to launch Microdroid "
                        + (protectedVm ? "pVM" : "VM")
                        + ". Check device logcat";
        try {
            microdroid =
                    MicrodroidBuilder.fromDevicePathWithPayloadBinaryName(
                                    apkPath, DEFAULT_PAYLOAD_BINARY_NAME)
                            .debugLevel("full")
                            .protectedVm(protectedVm)
                            .setAdbConnectTimeoutMs(timeoutMs)
                            .os(os)
                            .build(device);
            if (microdroid == null) {
                throw new DeviceNotAvailableException(errorMessage);
            }
            // Note: MicrodroidBuilder#build() checks this, but just in case.
            microdroid.waitForBootComplete(timeoutMs);
        } catch (Exception e) {
            throw new RuntimeException(errorMessage, e);
        } finally {
            if (microdroid != null) {
                device.shutdownMicrodroid(microdroid);
            }
        }
    }

    /** {@inheritDoc} */
    @Override
    public void setUp(TestInformation testInfo) throws DeviceNotAvailableException {
        ITestDevice device = testInfo.getDevice();
        if (!(device instanceof TestDevice)) {
            throw new DeviceNotAvailableException("Requires an actual TestDevice");
        }
        TestDevice testDevice = (TestDevice) device;
        String apkPath = getPathForPayloadPackage(testDevice);
        if (apkPath == null) {
            CLog.d("Unsupported device. Default payload not found. Skipping microdroid canary.");
            return;
        }

        String os = chooseOs(testDevice);
        ensureMicrodroidBoot(testDevice, apkPath, os, /* protectedVm= */ true);
        ensureMicrodroidBoot(testDevice, apkPath, os, /* protectedVm= */ false);
    }
}
