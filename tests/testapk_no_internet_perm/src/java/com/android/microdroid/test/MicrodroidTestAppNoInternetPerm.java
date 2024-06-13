/*
 * Copyright 2024 The Android Open Source Project
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

package com.android.microdroid.test;

import static com.google.common.truth.Truth.assertThat;

import static org.junit.Assert.assertThrows;

import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineManager;

import com.android.microdroid.test.device.MicrodroidDeviceTestBase;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;

/**
 * Test that the android.permission.MANAGE_VIRTUAL_MACHINE is enforced and that an app cannot launch
 * a VM without said permission.
 */
@RunWith(Parameterized.class)
public class MicrodroidTestAppNoInternetPerm extends MicrodroidDeviceTestBase {
    private static final String TAG = "MicrodroidTestAppNoInternetPerm";

    @Parameterized.Parameters(name = "protectedVm={0}")
    public static Object[] protectedVmConfigs() {
        return new Object[] {false, true};
    }

    @Parameterized.Parameter public boolean mProtectedVm;

    @Before
    public void setup() {
        prepareTestSetup(mProtectedVm, null);
    }

    @Test
    public void configuringNetworkSupportedRequiresInternetPermission() throws Exception {
        assumeSupportedDevice();
        assumeNonProtectedVM();
        assumeFeatureEnabled(VirtualMachineManager.FEATURE_NETWORK);

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setNetworkSupported(true)
                        .build();

        VirtualMachine vm =
                forceCreateNewVirtualMachine(
                        "config_network_supported_req_internet_permission", config);
        SecurityException e =
                assertThrows(
                        SecurityException.class, () -> runVmTestService(TAG, vm, (ts, tr) -> {}));
        assertThat(e).hasMessageThat().contains("android.permission.INTERNET permission");
    }
}
