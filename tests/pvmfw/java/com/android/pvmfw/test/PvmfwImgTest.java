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

import static org.junit.Assert.assertThrows;

import com.android.pvmfw.test.host.Pvmfw;
import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.DeviceRuntimeException;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.testtype.DeviceJUnit4ClassRunner;

import org.junit.Test;
import org.junit.runner.RunWith;

import java.util.Arrays;
import java.util.Collections;
import java.util.List;

/** Tests pvmfw.img and pvmfw */
@RunWith(DeviceJUnit4ClassRunner.class)
public class PvmfwImgTest extends CustomPvmfwHostTestCaseBase {
    @Test
    public void testConfigVersion1_0_boots() throws Exception {
        Pvmfw pvmfw = new Pvmfw.Builder(getPvmfwBinFile(), getBccFile()).setVersion(1, 0).build();
        pvmfw.serialize(getCustomPvmfwFile());

        launchProtectedVmAndWaitForBootCompleted(BOOT_COMPLETE_TIMEOUT_MS);
    }

    @Test
    public void testConfigVersion1_1_boots() throws Exception {
        Pvmfw pvmfw = new Pvmfw.Builder(getPvmfwBinFile(), getBccFile()).setVersion(1, 1).build();
        pvmfw.serialize(getCustomPvmfwFile());

        launchProtectedVmAndWaitForBootCompleted(BOOT_COMPLETE_TIMEOUT_MS);
    }

    @Test
    public void testInvalidConfigVersion_doesNotBoot() throws Exception {
        // Disclaimer: Update versions when they become valid
        List<int[]> invalid_versions =
                Arrays.asList(
                        new int[] {0, 0},
                        new int[] {0, 1},
                        new int[] {0, 0xFFFF},
                        new int[] {2, 0},
                        new int[] {2, 1},
                        new int[] {2, 0xFFFF},
                        new int[] {0xFFFF, 0},
                        new int[] {0xFFFF, 1},
                        new int[] {0xFFFF, 0xFFFF});

        Pvmfw.Builder builder = new Pvmfw.Builder(getPvmfwBinFile(), getBccFile());

        for (int[] pair : invalid_versions) {
            int major = pair[0];
            int minor = pair[1];
            String version = "v" + major + "." + minor;

            Pvmfw pvmfw = builder.setVersion(major, minor).build();
            pvmfw.serialize(getCustomPvmfwFile());

            assertThrows(
                    "pvmfw shouldn't boot with invalid version " + version,
                    DeviceRuntimeException.class,
                    () -> launchProtectedVmAndWaitForBootCompleted(BOOT_FAILURE_WAIT_TIME_MS));
        }
    }

    public ITestDevice launchProtectedVmAndWaitForBootCompleted(long adbTimeoutMs)
            throws DeviceNotAvailableException {
        return launchProtectedVmAndWaitForBootCompleted(
                MICRODROID_DEBUG_FULL, adbTimeoutMs, Collections.emptyMap());
    }
}
