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

package com.android.microdroid.test.preparer;

import com.android.tradefed.config.Option;
import com.android.tradefed.config.OptionClass;
import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.invoker.TestInformation;
import com.android.tradefed.log.LogUtil.CLog;
import com.android.tradefed.targetprep.BaseTargetPreparer;
import com.android.tradefed.targetprep.BuildError;
import com.android.tradefed.targetprep.TargetSetupError;

/**
 * Target preparer that disables microdroid's device policy for future VMs. This requires adb root
 * for configuring the relevant sysprop.
 *
 * <p>Will restore back to original value on tear down. adb will be also unrooted if it wasn't root.
 */
@OptionClass(alias = "disable-microdroid-debug-policy-preparer")
public final class DisableMicrodroidDebugPolicyPreparer extends BaseTargetPreparer {
    private static final String SYSPROP_CUSTOM_DEBUG_POLICY_PATH =
            "hypervisor.virtualizationmanager.debug_policy.path";

    private boolean mWasRoot = false;
    private String mOldDebugPolicyPath;

    @Option(
            name = "debug-policy-path",
            description = "Debug policy path for sysprop " + SYSPROP_CUSTOM_DEBUG_POLICY_PATH)
    private String mDebugPolicyPath = "/data/local/tmp/virt/stub_debug_policy.dts";

    @Override
    public void setUp(TestInformation testInfo)
            throws TargetSetupError, BuildError, DeviceNotAvailableException {
        ITestDevice device = testInfo.getDevice();
        mWasRoot = device.isAdbRoot();
        if (!mWasRoot && !device.enableAdbRoot()) {
            throw new TargetSetupError("Failed to adb root device", device.getDeviceDescriptor());
        }

        try {
            CLog.d("Bypassing micrdroid debug policy");
            mOldDebugPolicyPath = device.getProperty(SYSPROP_CUSTOM_DEBUG_POLICY_PATH);
            boolean result = device.setProperty(SYSPROP_CUSTOM_DEBUG_POLICY_PATH, mDebugPolicyPath);
            if (!result) {
                throw new TargetSetupError(
                        "Bypassing microdroid debug policy failed", device.getDeviceDescriptor());
            }
        } finally {
            if (!mWasRoot) {
                device.disableAdbRoot();
            }
        }
    }

    @Override
    public void tearDown(TestInformation testInfo, Throwable e) throws DeviceNotAvailableException {
        ITestDevice device = testInfo.getDevice();
        if (e instanceof DeviceNotAvailableException) {
            CLog.d("device not available: skipping teardown");
            return;
        }

        if (!mWasRoot) {
            device.enableAdbRoot();
        }

        CLog.d("Resetting microdroid debug policy");
        device.setProperty(
                SYSPROP_CUSTOM_DEBUG_POLICY_PATH,
                mOldDebugPolicyPath == null ? "" : mOldDebugPolicyPath);

        if (!mWasRoot) {
            device.disableAdbRoot();
        }
    }
}
