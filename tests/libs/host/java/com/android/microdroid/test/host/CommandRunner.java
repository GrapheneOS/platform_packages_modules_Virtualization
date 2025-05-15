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

import static org.hamcrest.CoreMatchers.is;
import static org.junit.Assert.fail;
import static org.junit.Assume.assumeThat;

import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.log.LogUtil.CLog;
import com.android.tradefed.util.CommandResult;
import com.android.tradefed.util.CommandStatus;

import java.util.Arrays;

import javax.annotation.Nonnull;

/** A helper class to provide easy way to run commands on a test device. */
public class CommandRunner {

    /** Default timeout. 30 sec because Microdroid is extremely slow on GCE-on-CF. */
    private static final long DEFAULT_TIMEOUT = 30000;

    private ITestDevice mDevice;

    public CommandRunner(@Nonnull ITestDevice device) {
        mDevice = device;
    }

    public ITestDevice getDevice() {
        return mDevice;
    }

    public String run(String... cmd) throws DeviceNotAvailableException {
        CommandResult result = runForResult(cmd);
        if (result.getStatus() != CommandStatus.SUCCESS) {
            fail(join(cmd) + " has failed: " + result);
        }
        return result.getStdout().trim();
    }

    public String tryRun(String... cmd) throws DeviceNotAvailableException {
        CommandResult result = runForResult(cmd);
        if (result.getStatus() == CommandStatus.SUCCESS) {
            return result.getStdout().trim();
        } else {
            CLog.d(join(cmd) + " has failed (but ok): " + result);
            return null;
        }
    }

    public String runWithTimeout(long timeoutMillis, String... cmd)
            throws DeviceNotAvailableException {
        CommandResult result = runForResultWithTimeout(timeoutMillis, cmd);
        if (result.getStatus() != CommandStatus.SUCCESS) {
            fail(join(cmd) + " has failed: " + result);
        }
        return result.getStdout().trim();
    }

    public CommandResult runForResultWithTimeout(long timeoutMillis, String... cmd)
            throws DeviceNotAvailableException {
        return mDevice.executeShellV2Command(
                join(cmd), timeoutMillis, java.util.concurrent.TimeUnit.MILLISECONDS);
    }

    public CommandResult runForResult(String... cmd) throws DeviceNotAvailableException {
        return mDevice.executeShellV2Command(join(cmd));
    }

    public void assumeSuccess(String... cmd) throws DeviceNotAvailableException {
        assumeThat(runForResult(cmd).getStatus(), is(CommandStatus.SUCCESS));
    }

    private static String join(String... strs) {
        return String.join(" ", Arrays.asList(strs));
    }
}
