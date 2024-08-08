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
import android.graphics.Rect;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig.AudioConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig.Disk;
import android.system.virtualmachine.VirtualMachineCustomImageConfig.DisplayConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig.GpuConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig.Partition;
import android.util.DisplayMetrics;
import android.view.WindowManager;
import android.view.WindowMetrics;

import com.google.gson.Gson;
import com.google.gson.annotations.SerializedName;

import java.io.FileReader;
import java.util.Arrays;

/** This class and its inner classes model vm_config.json. */
class ConfigJson {
    private static final boolean DEBUG = true;

    private ConfigJson() {}

    @SerializedName("protected")
    private boolean isProtected;

    private String name;
    private String cpu_topology;
    private String platform_version;
    private int memory_mib = 1024;
    private String console_input_device;
    private String bootloader;
    private String kernel;
    private String initrd;
    private String params;
    private boolean debuggable;
    private boolean console_out;
    private boolean connect_console;
    private boolean network;
    private InputJson input;
    private AudioJson audio;
    private DiskJson[] disks;
    private DisplayJson display;
    private GpuJson gpu;

    /** Parses JSON file at jsonPath */
    static ConfigJson from(String jsonPath) {
        try (FileReader r = new FileReader(jsonPath)) {
            return new Gson().fromJson(r, ConfigJson.class);
        } catch (Exception e) {
            throw new RuntimeException("Failed to parse " + jsonPath, e);
        }
    }

    private int getCpuTopology() {
        switch (cpu_topology) {
            case "one_cpu":
                return VirtualMachineConfig.CPU_TOPOLOGY_ONE_CPU;
            case "match_host":
                return VirtualMachineConfig.CPU_TOPOLOGY_MATCH_HOST;
            default:
                throw new RuntimeException("invalid cpu topology: " + cpu_topology);
        }
    }

    private int getDebugLevel() {
        return debuggable
                ? VirtualMachineConfig.DEBUG_LEVEL_FULL
                : VirtualMachineConfig.DEBUG_LEVEL_NONE;
    }

    /** Converts this parsed JSON into VirtualMachieConfig */
    VirtualMachineConfig toConfig(Context context) {
        return new VirtualMachineConfig.Builder(context)
                .setProtectedVm(isProtected)
                .setMemoryBytes((long) memory_mib * 1024 * 1024)
                .setConsoleInputDevice(console_input_device)
                .setCpuTopology(getCpuTopology())
                .setCustomImageConfig(toCustomImageConfig(context))
                .setDebugLevel(getDebugLevel())
                .setVmOutputCaptured(console_out)
                .setConnectVmConsole(connect_console)
                .build();
    }

    private VirtualMachineCustomImageConfig toCustomImageConfig(Context context) {
        VirtualMachineCustomImageConfig.Builder builder =
                new VirtualMachineCustomImageConfig.Builder();

        builder.setName(name)
                .setBootloaderPath(bootloader)
                .setKernelPath(kernel)
                .setInitrdPath(initrd)
                .useNetwork(network);

        if (input != null) {
            builder.useTouch(input.touchscreen)
                    .useKeyboard(input.keyboard)
                    .useMouse(input.mouse)
                    .useTrackpad(input.trackpad)
                    .useSwitches(input.switches);
        }

        if (audio != null) {
            builder.setAudioConfig(audio.toConfig());
        }

        if (display != null) {
            builder.setDisplayConfig(display.toConfig(context));
        }

        if (gpu != null) {
            builder.setGpuConfig(gpu.toConfig());
        }

        if (params != null) {
            Arrays.stream(params.split(" ")).forEach(builder::addParam);
        }

        if (disks != null) {
            Arrays.stream(disks).map(d -> d.toConfig()).forEach(builder::addDisk);
        }

        return builder.build();
    }

    private static class InputJson {
        private InputJson() {}

        private boolean touchscreen;
        private boolean keyboard;
        private boolean mouse;
        private boolean switches;
        private boolean trackpad;
    }

    private static class AudioJson {
        private AudioJson() {}

        private boolean microphone;
        private boolean speaker;

        private AudioConfig toConfig() {
            return new AudioConfig.Builder()
                    .setUseMicrophone(microphone)
                    .setUseSpeaker(speaker)
                    .build();
        }
    }

    private static class DiskJson {
        private DiskJson() {}

        private boolean writable;
        private String image;
        private PartitionJson[] partitions;

        private Disk toConfig() {
            Disk d = writable ? Disk.RWDisk(image) : Disk.RODisk(image);
            for (PartitionJson pj : partitions) {
                boolean writable = this.writable && pj.writable;
                d.addPartition(new Partition(pj.label, pj.path, writable, pj.guid));
            }
            return d;
        }
    }

    private static class PartitionJson {
        private PartitionJson() {}

        private boolean writable;
        private String label;
        private String path;
        private String guid;
    }

    private static class DisplayJson {
        private DisplayJson() {}

        private float scale;
        private int refresh_rate;
        private int width_pixels;
        private int height_pixels;

        private DisplayConfig toConfig(Context context) {
            WindowManager wm = context.getSystemService(WindowManager.class);
            WindowMetrics metrics = wm.getCurrentWindowMetrics();
            Rect dispBounds = metrics.getBounds();

            int width = width_pixels > 0 ? width_pixels : dispBounds.right;
            int height = height_pixels > 0 ? height_pixels : dispBounds.bottom;

            int dpi = (int) (DisplayMetrics.DENSITY_DEFAULT * metrics.getDensity());
            if (scale > 0.0f) {
                dpi = (int) (dpi * scale);
            }

            int refreshRate = (int) context.getDisplay().getRefreshRate();
            if (this.refresh_rate != 0) {
                refreshRate = this.refresh_rate;
            }

            return new DisplayConfig.Builder()
                    .setWidth(width)
                    .setHeight(height)
                    .setHorizontalDpi(dpi)
                    .setVerticalDpi(dpi)
                    .setRefreshRate(refreshRate)
                    .build();
        }
    }

    private static class GpuJson {
        private GpuJson() {}

        private String backend;
        private String pci_address;
        private String renderer_features;
        private boolean renderer_use_egl = true;
        private boolean renderer_use_gles = true;
        private boolean renderer_use_glx = false;
        private boolean renderer_use_surfaceless = true;
        private boolean renderer_use_vulkan = false;
        private String[] context_types;

        private GpuConfig toConfig() {
            return new GpuConfig.Builder()
                    .setBackend(backend)
                    .setPciAddress(pci_address)
                    .setRendererFeatures(renderer_features)
                    .setRendererUseEgl(renderer_use_egl)
                    .setRendererUseGles(renderer_use_gles)
                    .setRendererUseGlx(renderer_use_glx)
                    .setRendererUseSurfaceless(renderer_use_surfaceless)
                    .setRendererUseVulkan(renderer_use_vulkan)
                    .setContextTypes(context_types)
                    .build();
        }
    }
}
