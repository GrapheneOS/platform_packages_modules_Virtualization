/*
 * Copyright (C) 2024 The Android Open Source Project
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

package com.android.virtualization.vmlauncher;

import android.content.Context;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineCallback;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig;
import android.system.virtualmachine.VirtualMachineException;
import android.system.virtualmachine.VirtualMachineManager;
import android.util.Log;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ForkJoinPool;

/** Utility class for creating a VM and waiting for it to finish. */
class Runner {
    private static final String TAG = Runner.class.getSimpleName();
    private final VirtualMachine mVirtualMachine;
    private final Callback mCallback;

    private Runner(VirtualMachine vm, Callback cb) {
        mVirtualMachine = vm;
        mCallback = cb;
    }

    /** Create a virtual machine of the given config, under the given context. */
    static Runner create(Context context, VirtualMachineConfig config)
            throws VirtualMachineException {
        // context may already be the app context, but calling this again is not harmful.
        // See b/359439878 on why vmm should be obtained from the app context.
        Context appContext = context.getApplicationContext();
        VirtualMachineManager vmm = appContext.getSystemService(VirtualMachineManager.class);
        VirtualMachineCustomImageConfig customConfig = config.getCustomImageConfig();
        if (customConfig == null) {
            throw new RuntimeException("CustomImageConfig is missing");
        }

        String name = customConfig.getName();
        if (name == null || name.isEmpty()) {
            throw new RuntimeException("Virtual machine's name is missing in the config");
        }

        VirtualMachine vm = vmm.getOrCreate(name, config);
        try {
            vm.setConfig(config);
        } catch (VirtualMachineException e) {
            vmm.delete(name);
            vm = vmm.create(name, config);
            Log.w(TAG, "Re-creating virtual machine (" + name + ")", e);
        }

        Callback cb = new Callback();
        vm.setCallback(ForkJoinPool.commonPool(), cb);
        vm.run();
        return new Runner(vm, cb);
    }

    /** Give access to the underlying VirtualMachine object. */
    VirtualMachine getVm() {
        return mVirtualMachine;
    }

    /** Get future about VM's exit status. */
    CompletableFuture<Boolean> getExitStatus() {
        return mCallback.mFinishedSuccessfully;
    }

    private static class Callback implements VirtualMachineCallback {
        final CompletableFuture<Boolean> mFinishedSuccessfully = new CompletableFuture<>();

        @Override
        public void onPayloadStarted(VirtualMachine vm) {
            // This event is only from Microdroid-based VM. Custom VM shouldn't emit this.
        }

        @Override
        public void onPayloadReady(VirtualMachine vm) {
            // This event is only from Microdroid-based VM. Custom VM shouldn't emit this.
        }

        @Override
        public void onPayloadFinished(VirtualMachine vm, int exitCode) {
            // This event is only from Microdroid-based VM. Custom VM shouldn't emit this.
        }

        @Override
        public void onError(VirtualMachine vm, int errorCode, String message) {
            Log.e(TAG, "Error from VM. code: " + errorCode + " (" + message + ")");
            mFinishedSuccessfully.complete(false);
        }

        @Override
        public void onStopped(VirtualMachine vm, int reason) {
            Log.d(TAG, "VM stopped. Reason: " + reason);
            mFinishedSuccessfully.complete(true);
        }
    }
}
