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

import static android.system.virtualmachine.VirtualMachineConfig.CPU_TOPOLOGY_MATCH_HOST;

import android.app.Activity;
import android.os.Bundle;
import android.system.virtualmachine.VirtualMachineCustomImageConfig;
import android.util.Log;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineCallback;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineException;
import android.system.virtualmachine.VirtualMachineManager;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class MainActivity extends Activity {
    private static final String TAG = "VmLauncherApp";
    private static final String VM_NAME = "my_custom_vm";
    private static final boolean DEBUG = true;
    private final ExecutorService mExecutorService = Executors.newFixedThreadPool(4);
    private VirtualMachine mVirtualMachine;

    private VirtualMachineConfig createVirtualMachineConfig(String jsonPath) {
        VirtualMachineConfig.Builder configBuilder =
                new VirtualMachineConfig.Builder(getApplication());
        configBuilder.setCpuTopology(CPU_TOPOLOGY_MATCH_HOST);

        configBuilder.setProtectedVm(false);
        if (DEBUG) {
            configBuilder.setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL);
            configBuilder.setVmOutputCaptured(true);
        }
        VirtualMachineCustomImageConfig.Builder customImageConfigBuilder =
                new VirtualMachineCustomImageConfig.Builder();
        try {
            String rawJson = new String(Files.readAllBytes(Path.of(jsonPath)));
            JSONObject json = new JSONObject(rawJson);
            customImageConfigBuilder.setName(json.optString("name", ""));
            if (json.has("kernel")) {
                customImageConfigBuilder.setKernelPath(json.getString("kernel"));
            }
            if (json.has("initrd")) {
                customImageConfigBuilder.setInitrdPath(json.getString("initrd"));
            }
            if (json.has("params")) {
                Arrays.stream(json.getString("params").split(" "))
                        .forEach(customImageConfigBuilder::addParam);
            }
            if (json.has("bootloader")) {
                customImageConfigBuilder.setInitrdPath(json.getString("bootloader"));
            }
            if (json.has("disks")) {
                JSONArray diskArr = json.getJSONArray("disks");
                for (int i = 0; i < diskArr.length(); i++) {
                    JSONObject item = diskArr.getJSONObject(i);
                    if (item.has("image")) {
                        if (item.optBoolean("writable", false)) {
                            customImageConfigBuilder.addDisk(
                                    VirtualMachineCustomImageConfig.Disk.RWDisk(
                                            item.getString("image")));
                        } else {
                            customImageConfigBuilder.addDisk(
                                    VirtualMachineCustomImageConfig.Disk.RODisk(
                                            item.getString("image")));
                        }
                    }
                }
            }

            configBuilder.setMemoryBytes(8L * 1024 * 1024 * 1024 /* 8 GB */);
            configBuilder.setCustomImageConfig(customImageConfigBuilder.build());

        } catch (JSONException | IOException e) {
            throw new IllegalStateException("malformed input", e);
        }
        return configBuilder.build();
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_main);
        VirtualMachineCallback callback =
                new VirtualMachineCallback() {
                    // store reference to ExecutorService to avoid race condition
                    private final ExecutorService mService = mExecutorService;

                    @Override
                    public void onPayloadStarted(VirtualMachine vm) {
                        Log.e(TAG, "payload start");
                    }

                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        // This check doesn't 100% prevent race condition or UI hang.
                        // However, it's fine for demo.
                        if (mService.isShutdown()) {
                            return;
                        }
                        Log.d(TAG, "(Payload is ready. Testing VM service...)");
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        // This check doesn't 100% prevent race condition, but is fine for demo.
                        if (!mService.isShutdown()) {
                            Log.d(
                                    TAG,
                                    String.format("(Payload finished. exit code: %d)", exitCode));
                        }
                    }

                    @Override
                    public void onError(VirtualMachine vm, int errorCode, String message) {
                        Log.d(
                                TAG,
                                String.format(
                                        "(Error occurred. code: %d, message: %s)",
                                        errorCode, message));
                    }

                    @Override
                    public void onStopped(VirtualMachine vm, int reason) {
                        Log.e(TAG, "vm stop");
                    }
                };

        try {
            VirtualMachineConfig config =
                    createVirtualMachineConfig("/data/local/tmp/vm_config.json");
            VirtualMachineManager vmm =
                    getApplication().getSystemService(VirtualMachineManager.class);
            if (vmm == null) {
                Log.e(TAG, "vmm is null");
                return;
            }
            mVirtualMachine = vmm.getOrCreate(VM_NAME, config);
            try {
                mVirtualMachine.setConfig(config);
            } catch (VirtualMachineException e) {
                vmm.delete(VM_NAME);
                mVirtualMachine = vmm.create(VM_NAME, config);
                Log.e(TAG, "error" + e);
            }

            Log.d(TAG, "vm start");
            mVirtualMachine.run();
            mVirtualMachine.setCallback(Executors.newSingleThreadExecutor(), callback);
            if (DEBUG) {
                InputStream console = mVirtualMachine.getConsoleOutput();
                InputStream log = mVirtualMachine.getLogOutput();
                mExecutorService.execute(new Reader("console", console));
                mExecutorService.execute(new Reader("log", log));
            }
        } catch (VirtualMachineException e) {
            throw new RuntimeException(e);
        }
    }

    /** Reads data from an input stream and posts it to the output data */
    static class Reader implements Runnable {
        private final String mName;
        private final InputStream mStream;

        Reader(String name, InputStream stream) {
            mName = name;
            mStream = stream;
        }

        @Override
        public void run() {
            try {
                BufferedReader reader = new BufferedReader(new InputStreamReader(mStream));
                String line;
                while ((line = reader.readLine()) != null && !Thread.interrupted()) {
                    Log.d(TAG, mName + ": " + line);
                }
            } catch (IOException e) {
                Log.e(TAG, "Exception while posting " + mName + " output: " + e.getMessage());
            }
        }
    }
}
