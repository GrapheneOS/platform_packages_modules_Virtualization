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

package com.android.microdroid.benchmark;

import static com.google.common.truth.Truth.assertWithMessage;

import android.os.RemoteException;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineException;
import android.util.Log;

import com.android.microdroid.test.device.MicrodroidDeviceTestBase.VmEventListener;
import com.android.microdroid.testservice.IBenchmarkService;

/**
 * This VM listener is used in {@link MicrodroidBenchmark} tests to facilitate the communication
 * between the host and VM via {@link IBenchmarkService}.
 */
class BenchmarkVmListener extends VmEventListener {
    private static final String TAG = "BenchmarkVm";

    private boolean mPayloadReady;

    interface InnerListener {
        /** This is invoked when both the payload and {@link IBenchmarkService} are ready. */
        void onPayloadReady(VirtualMachine vm, IBenchmarkService benchmarkService)
                throws RemoteException;
    }

    private final InnerListener mListener;

    private BenchmarkVmListener(InnerListener listener) {
        mListener = listener;
    }

    @Override
    public final void onPayloadReady(VirtualMachine vm) {
        mPayloadReady = true;
        try {
            IBenchmarkService benchmarkService =
                    IBenchmarkService.Stub.asInterface(
                            vm.connectToVsockServer(IBenchmarkService.PORT));
            assertWithMessage("Benchmark service is null").that(benchmarkService).isNotNull();

            mListener.onPayloadReady(vm, benchmarkService);
        } catch (Exception e) {
            Log.e(TAG, "Error inside onPayloadReady():" + e);
            throw new RuntimeException(e);
        }
        forceStop(vm);
    }

    @Override
    public void onError(VirtualMachine vm, int errorCode, String message) {
        throw new RuntimeException("Error while benchmark (" + errorCode + "): " + message);
    }

    static BenchmarkVmListener create(InnerListener listener) {
        return new BenchmarkVmListener(listener);
    }

    @Override
    public void runToFinish(String logTag, VirtualMachine vm)
            throws VirtualMachineException, InterruptedException {
        mPayloadReady = false;
        super.runToFinish(logTag, vm);
        assertWithMessage("Payload was never ready").that(mPayloadReady).isTrue();
    }
}
