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

package com.android.microdroid.test.host;

import static com.android.tradefed.testtype.DeviceJUnit4ClassRunner.TestLogData;

import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.result.FileInputStreamSource;
import com.android.tradefed.result.LogDataType;

import java.io.File;

/** A helper class for archiving device log files to the host's tradefed output directory. */
public abstract class LogArchiver {
    /**
     * Copy device log (then delete) to a tradefed output directory on the host.
     *
     * @param logs A {@link TestLogData} that needs to be owned by the actual test case.
     * @param device The device to pull the log file from.
     * @param remotePath The path on the device.
     * @param localName Local file name to be copied to.
     */
    public static void archiveLogThenDelete(
            TestLogData logs, ITestDevice device, String remotePath, String localName)
            throws DeviceNotAvailableException {
        File logFile = device.pullFile(remotePath);
        if (logFile != null) {
            logs.addTestLog(localName, LogDataType.TEXT, new FileInputStreamSource(logFile));
            // Delete to avoid confusing logs from a previous run, just in case.
            device.deleteFile(remotePath);
        }
    }
}
