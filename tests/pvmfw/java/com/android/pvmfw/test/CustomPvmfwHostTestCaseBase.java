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

package com.android.pvmfw.test;

import static com.android.tradefed.device.TestDevice.MicrodroidBuilder;

import static com.google.common.truth.Truth.assertThat;

import static org.junit.Assume.assumeTrue;

import androidx.annotation.NonNull;
import androidx.annotation.Nullable;

import com.android.microdroid.test.host.MicrodroidHostTestCaseBase;
import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.device.TestDevice;
import com.android.tradefed.util.FileUtil;

import org.junit.After;
import org.junit.Before;

import java.io.File;
import java.util.Map;

/** Base class for testing custom pvmfw */
public class CustomPvmfwHostTestCaseBase extends MicrodroidHostTestCaseBase {
    @NonNull public static final String PVMFW_FILE_NAME = "pvmfw_test.bin";
    @NonNull public static final String BCC_FILE_NAME = "bcc.dat";
    @NonNull public static final String PACKAGE_FILE_NAME = "MicrodroidTestApp.apk";
    @NonNull public static final String PACKAGE_NAME = "com.android.microdroid.test";
    @NonNull public static final String MICRODROID_DEBUG_FULL = "full";
    @NonNull public static final String MICRODROID_DEBUG_NONE = "none";

    @NonNull public static final String MICRODROID_CONFIG_PATH = "assets/vm_config_apex.json";

    @NonNull
    public static final String VM_REFERENCE_DT_PATH = "/data/local/tmp/pvmfw/reference_dt.dtb";

    @NonNull public static final String MICRODROID_LOG_PATH = TEST_ROOT + "log.txt";
    public static final int BOOT_COMPLETE_TIMEOUT_MS = 30000; // 30 seconds
    public static final int BOOT_FAILURE_WAIT_TIME_MS = 10000; // 10 seconds
    public static final int CONSOLE_OUTPUT_WAIT_MS = 5000; // 5 seconds

    @NonNull public static final String CUSTOM_PVMFW_FILE_PREFIX = "pvmfw";
    @NonNull public static final String CUSTOM_PVMFW_FILE_SUFFIX = ".bin";

    @NonNull
    public static final String CUSTOM_PVMFW_IMG_PATH = TRADEFED_TEST_ROOT + PVMFW_FILE_NAME;

    @NonNull public static final String CUSTOM_PVMFW_IMG_PATH_PROP = "hypervisor.pvmfw.path";

    @Nullable private File mPvmfwBinFileOnHost;
    @Nullable private File mBccFileOnHost;
    @Nullable private File mVmReferenceDtFile;
    private boolean mSecretKeeperSupported;

    @NonNull private TestDevice mAndroidDevice;
    @Nullable private ITestDevice mMicrodroidDevice;

    @Nullable private File mCustomPvmfwFileOnHost;

    @Before
    public void setUp() throws Exception {
        mAndroidDevice = (TestDevice) getDevice();

        // Check device capabilities
        assumeDeviceIsCapable(mAndroidDevice);
        assumeTrue(
                "Skip if protected VMs are not supported",
                mAndroidDevice.supportsMicrodroid(/* protectedVm= */ true));

        mPvmfwBinFileOnHost = findTestFile(PVMFW_FILE_NAME);
        mBccFileOnHost = findTestFile(BCC_FILE_NAME);

        // This is prepared by AndroidTest.xml
        mVmReferenceDtFile = mAndroidDevice.pullFile(VM_REFERENCE_DT_PATH);

        mSecretKeeperSupported = isUpdatableVmSupported();

        // Prepare for system properties for custom pvmfw.img.
        // File will be prepared later in individual test and then pushed to device
        // when launching with launchProtectedVmAndWaitForBootCompleted().
        mCustomPvmfwFileOnHost =
                FileUtil.createTempFile(CUSTOM_PVMFW_FILE_PREFIX, CUSTOM_PVMFW_FILE_SUFFIX);
        setPropertyOrThrow(mAndroidDevice, CUSTOM_PVMFW_IMG_PATH_PROP, CUSTOM_PVMFW_IMG_PATH);

        // Prepare for launching microdroid
        mAndroidDevice.installPackage(findTestFile(PACKAGE_FILE_NAME), /* reinstall */ false);
        prepareVirtualizationTestSetup(mAndroidDevice);
        mMicrodroidDevice = null;
    }

    @After
    public void shutdown() throws Exception {
        shutdownMicrodroid();

        mAndroidDevice.uninstallPackage(PACKAGE_NAME);

        FileUtil.deleteFile(mVmReferenceDtFile);

        // Cleanup for custom pvmfw.img
        setPropertyOrThrow(mAndroidDevice, CUSTOM_PVMFW_IMG_PATH_PROP, "");
        FileUtil.deleteFile(mCustomPvmfwFileOnHost);

        cleanUpVirtualizationTestSetup(mAndroidDevice);
    }

    /** Returns android device */
    @NonNull
    public TestDevice getAndroidDevice() {
        return mAndroidDevice;
    }

    /** Returns pvmfw.bin file on host for building custom pvmfw with */
    @NonNull
    public File getPvmfwBinFile() {
        return mPvmfwBinFileOnHost;
    }

    /** Returns BCC file on host for building custom pvmfw with */
    @NonNull
    public File getBccFile() {
        return mBccFileOnHost;
    }

    /** Returns VM reference DT, generated from DUT, on host for building custom pvmfw with. */
    @Nullable
    public File getVmReferenceDtFile() {
        return mVmReferenceDtFile;
    }

    /**
     * Returns a custom pvmfw file.
     *
     * <p>This is a temporary file on host. The file should been prepared as a custom pvmfw because
     * calling {@link #launchProtectedVmAndWaitForBootCompleted}, so virtualization manager can read
     * the file path from sysprop and boot pVM with it.
     */
    @NonNull
    public File getCustomPvmfwFile() {
        return mCustomPvmfwFileOnHost;
    }

    /**
     * Returns whether a secretkeeper is supported.
     *
     * <p>If {@code true}, then VM reference DT must exist. (i.e. {@link #getVmReferenceDtFile} must
     * exist {@code null}).
     */
    public boolean isSecretKeeperSupported() {
        return mSecretKeeperSupported;
    }

    /**
     * Launches protected VM with custom pvmfw ({@link #getCustomPvmfwFile}) and wait for boot
     * completed. Throws exception when boot failed.
     */
    public ITestDevice launchProtectedVmAndWaitForBootCompleted(
            String debugLevel, long adbTimeoutMs, @NonNull Map<String, File> bootFiles)
            throws DeviceNotAvailableException {
        MicrodroidBuilder builder =
                MicrodroidBuilder.fromDevicePath(
                                getPathForPackage(PACKAGE_NAME), MICRODROID_CONFIG_PATH)
                        .debugLevel(debugLevel)
                        .protectedVm(/* protectedVm= */ true)
                        .addBootFile(mCustomPvmfwFileOnHost, PVMFW_FILE_NAME)
                        .setAdbConnectTimeoutMs(adbTimeoutMs);
        for (String name : bootFiles.keySet()) {
            File file = bootFiles.get(name);
            builder.addBootFile(file, name);
        }

        mMicrodroidDevice = builder.build(mAndroidDevice);

        assertThat(mMicrodroidDevice.waitForBootComplete(BOOT_COMPLETE_TIMEOUT_MS)).isTrue();
        assertThat(mMicrodroidDevice.enableAdbRoot()).isTrue();
        return mMicrodroidDevice;
    }

    /** Shuts down microdroid if it's running */
    public void shutdownMicrodroid() throws Exception {
        if (mMicrodroidDevice != null) {
            mAndroidDevice.shutdownMicrodroid(mMicrodroidDevice);
            mMicrodroidDevice = null;
        }
    }
}
