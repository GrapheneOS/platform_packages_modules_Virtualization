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

import android.system.virtualmachine.VirtualMachineConfig;

import com.android.compatibility.common.util.CddTest;
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
public class MicrodroidTestAppNoPerm extends MicrodroidDeviceTestBase {

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
    @CddTest(
            requirements = {
                "9.17/C-1-1",
                "9.17/C-1-2",
                "9.17/C-1-4",
            })
    public void createVmRequiresPermission() {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so").build();

        SecurityException e =
                assertThrows(
                        SecurityException.class,
                        () -> forceCreateNewVirtualMachine("test_vm_requires_permission", config));
        assertThat(e)
                .hasMessageThat()
                .contains("android.permission.MANAGE_VIRTUAL_MACHINE permission");
    }
}
