/*
 * Copyright 2021 The Android Open Source Project
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
package android.system.virtualizationservice;

import android.system.virtualizationservice.CpuTopology;
import android.system.virtualizationservice.VirtualMachinePayloadConfig;

/** Configuration for running an App in a VM */
parcelable VirtualMachineAppConfig {
    /** Name of VM */
    String name;

    /** Id of the VM instance */
    byte[64] instanceId;

    /** Main APK */
    ParcelFileDescriptor apk;

    /** idsig for an APK */
    ParcelFileDescriptor idsig;

    /** Idsigs for the extra APKs. Must match with the extra_apks in the payload config. */
    List<ParcelFileDescriptor> extraIdsigs;

    /** instance.img that has per-instance data */
    ParcelFileDescriptor instanceImage;

    /**
     * This backs the persistent, encrypted storage in vm.
     * It also comes with some integrity guarantees.
     * Note: Storage is an optional feature
     */
    @nullable ParcelFileDescriptor encryptedStorageImage;

    union Payload {
        /**
         * Path to a JSON file in an APK containing the configuration.
         *
         * <p>Setting this field requires android.permission.USE_CUSTOM_VIRTUAL_MACHINE
         */
        @utf8InCpp String configPath;

        /**
         * Configuration provided explicitly.
         */
        VirtualMachinePayloadConfig payloadConfig;
    }

    /** Detailed configuration for the VM, specifying how the payload will be run. */
    Payload payload;

    /**
     * Name of the OS to run the payload. Currently "microdroid" and
     * "microdroid_gki-android14-6.1" is supported.
     *
     * <p>Setting this field to a value other than "microdroid" requires
     * android.permission.USE_CUSTOM_VIRTUAL_MACHINE
     */
    @utf8InCpp String osName = "microdroid";

    enum DebugLevel {
        /** Not debuggable at all */
        NONE,
        /**
         * Fully debuggable. All logs are shown, kernel messages are shown, and adb shell is
         * supported
         */
        FULL,
    }

    /** Debug level of the VM */
    DebugLevel debugLevel = DebugLevel.NONE;

    /** Whether the VM should be a protected VM. */
    boolean protectedVm;

    /**
     * The amount of RAM to give the VM, in MiB. If this is 0 or negative then it will default to
     * the value in microdroid.json, if any, or the crosvm default.
     */
    int memoryMib;

    /** The vCPU topology that will be generated for the VM. Default to 1 vCPU. */
    CpuTopology cpuTopology = CpuTopology.ONE_CPU;

    /**
     * Encapsulates parameters that require android.permission.USE_CUSTOM_VIRTUAL_MACHINE.
     */
    parcelable CustomConfig {
        /**
         * If specified, boot Microdroid VM with the given kernel.
         *
         */
        @nullable ParcelFileDescriptor customKernelImage;

        /**
         * Port at which crosvm will start a gdb server to debug guest kernel.
         * If set to zero, then gdb server won't be started.
         *
         */
        int gdbPort = 0;

        /** A disk image containing vendor specific modules. */
        @nullable ParcelFileDescriptor vendorImage;

        /** List of SysFS nodes of devices to be assigned */
        String[] devices;

        /**
         * Whether the VM should be able to keep its secret when updated, if possible. This
         * should rarely need to be set false.
         */
        boolean wantUpdatable = true;

        /** Whether the VM should have network feature. */
        boolean networkSupported;

        /** Additional parameters to pass to the VM's kernel cmdline. */
        String[] extraKernelCmdlineParams;
    }

    /** Configuration parameters guarded by android.permission.USE_CUSTOM_VIRTUAL_MACHINE */
    @nullable CustomConfig customConfig;

    /**
     *  Ask the kernel for transparent huge-pages (THP). This is only a hint and
     *  the kernel will allocate THP-backed memory only if globally enabled by
     *  the system and if any can be found. See
     *  https://docs.kernel.org/admin-guide/mm/transhuge.html
     */
    boolean hugePages;

    /** Enable boost UClamp for less variance during testing/benchmarking */
    boolean boostUclamp;
}
