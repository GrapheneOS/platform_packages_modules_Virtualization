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
package com.android.microdroid.test;

import static android.system.virtualmachine.VirtualMachine.STATUS_DELETED;
import static android.system.virtualmachine.VirtualMachine.STATUS_RUNNING;
import static android.system.virtualmachine.VirtualMachine.STATUS_STOPPED;
import static android.system.virtualmachine.VirtualMachineConfig.CPU_TOPOLOGY_MATCH_HOST;
import static android.system.virtualmachine.VirtualMachineConfig.CPU_TOPOLOGY_ONE_CPU;
import static android.system.virtualmachine.VirtualMachineConfig.DEBUG_LEVEL_FULL;
import static android.system.virtualmachine.VirtualMachineConfig.DEBUG_LEVEL_NONE;
import static android.system.virtualmachine.VirtualMachineManager.CAPABILITY_NON_PROTECTED_VM;
import static android.system.virtualmachine.VirtualMachineManager.CAPABILITY_PROTECTED_VM;

import static com.android.system.virtualmachine.flags.Flags.promoteSetShouldUseHugepagesToSystemApi;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.Truth.assertWithMessage;
import static com.google.common.truth.TruthJUnit.assume;

import static org.junit.Assert.assertThrows;
import static org.junit.Assume.assumeFalse;
import static org.junit.Assume.assumeTrue;

import static java.nio.file.StandardCopyOption.REPLACE_EXISTING;
import static java.util.stream.Collectors.toList;

import android.app.Instrumentation;
import android.app.UiAutomation;
import android.content.ComponentName;
import android.content.Context;
import android.content.ContextWrapper;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.IBinder;
import android.os.Parcel;
import android.os.ParcelFileDescriptor;
import android.os.ParcelFileDescriptor.AutoCloseInputStream;
import android.os.ParcelFileDescriptor.AutoCloseOutputStream;
import android.os.SystemProperties;
import android.platform.test.annotations.RequiresFlagsEnabled;
import android.system.OsConstants;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineCallback;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineDescriptor;
import android.system.virtualmachine.VirtualMachineException;
import android.system.virtualmachine.VirtualMachineManager;
import android.util.Log;

import androidx.test.platform.app.InstrumentationRegistry;

import com.android.compatibility.common.util.CddTest;
import com.android.compatibility.common.util.GmsTest;
import com.android.compatibility.common.util.VsrTest;
import com.android.microdroid.test.device.MicrodroidDeviceTestBase;
import com.android.microdroid.test.vmshare.IVmShareTestService;
import com.android.microdroid.testservice.IAppCallback;
import com.android.microdroid.testservice.ITestService;
import com.android.microdroid.testservice.IVmCallback;
import com.android.system.virtualmachine.flags.Flags;
import com.android.virt.vm_attestation.testservice.IAttestationService.AttestationStatus;
import com.android.virt.vm_attestation.testservice.IAttestationService.SigningResult;
import com.android.virt.vm_attestation.util.X509Utils;

import com.google.common.base.Strings;
import com.google.common.truth.BooleanSubject;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.function.ThrowingRunnable;
import org.junit.rules.Timeout;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.io.OutputStreamWriter;
import java.io.RandomAccessFile;
import java.io.Writer;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.security.cert.X509Certificate;
import java.time.LocalDateTime;
import java.time.format.DateTimeFormatter;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.List;
import java.util.OptionalLong;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import java.util.stream.Stream;

@RunWith(Parameterized.class)
public class MicrodroidTests extends MicrodroidDeviceTestBase {
    private static final String TAG = "MicrodroidTests";
    private static final String TEST_APP_PACKAGE_NAME = "com.android.microdroid.test";
    private static final String VM_ATTESTATION_PAYLOAD_PATH = "libvm_attestation_test_payload.so";
    private static final String TEST_TENANT_APK_NAME = "apk:com.android.microdroid.test";

    private static final String VM_ATTESTATION_MESSAGE = "Hello RKP from AVF!";
    private static final long TOLERANCE_BYTES = 400_000;
    private static final int ENCRYPTED_STORAGE_BYTES = 4_000_000;

    private static final String RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME =
            "com.android.microdroid.test_relaxed_rollback_protection_scheme";

    private static final String ENCRYPTED_STORE_KEK_ON_CE_TEST_PACKAGE_NAME =
            "com.android.microdroid.test_enc_store_kek_on_ce";

    @Rule public Timeout globalTimeout = Timeout.seconds(300);

    @Parameterized.Parameters(name = "protectedVm={0},os={1}")
    public static Collection<Object[]> params() {
        List<Object[]> ret = new ArrayList<>();
        // TODO(b/302465542): run only the latest GKI on presubmit to reduce running time
        for (String os : SUPPORTED_OSES) {
            ret.add(new Object[] {true /* protectedVm */, os});
            ret.add(new Object[] {false /* protectedVm */, os});
        }
        return ret;
    }

    @Parameterized.Parameter(0)
    public boolean mProtectedVm;

    @Parameterized.Parameter(1)
    public String mOs;

    @Before
    public void setup() {
        prepareTestSetup(mProtectedVm, mOs);
        if (mOs != "microdroid") {
            // Using a non-default VM always needs the custom permission.
            grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        } else {
            // USE_CUSTOM_VIRTUAL_MACHINE permission has protection level signature|development,
            // meaning that it will be automatically granted when test apk is installed.
            // But most callers shouldn't need this permission, so by default we run tests with it
            // revoked.
            // Tests that rely on the state of the permission should explicitly grant or revoke it.
            revokePermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        }
    }

    @After
    public void tearDown() {
        deleteAllExistingVMsByApp();
        revokePermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        // Some tests might install additional apks, so we need to clean them up here.
        uninstallApp(RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME);
        uninstallApp(ENCRYPTED_STORE_KEK_ON_CE_TEST_PACKAGE_NAME);
    }

    private static final String EXAMPLE_STRING = "Literally any string!! :)";

    private static final String VM_SHARE_APP_PACKAGE_NAME = "com.android.microdroid.vmshare_app";

    private static final int FIRST_TENANT_UID = 10000;

    private void createAndConnectToVmHelper(int cpuTopology, boolean shouldUseHugepages)
            throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig.Builder builder =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setCpuTopology(cpuTopology);
        if (promoteSetShouldUseHugepagesToSystemApi()) {
            builder.setShouldUseHugepages(shouldUseHugepages);
        }
        VirtualMachineConfig config = builder.build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mAddInteger = ts.addInteger(123, 456);
                            tr.mAppRunProp = ts.readProperty("debug.microdroid.app.run");
                            tr.mSublibRunProp = ts.readProperty("debug.microdroid.app.sublib.run");
                            tr.mApkContentsPath = ts.getApkContentsPath();
                            tr.mEncryptedStoragePath = ts.getEncryptedStoragePath();
                            tr.mInstanceSecret = ts.insecurelyExposeVmInstanceSecret();
                        });
        testResults.assertNoException();
        assertThat(testResults.mAddInteger).isEqualTo(123 + 456);
        assertThat(testResults.mAppRunProp).isEqualTo("true");
        assertThat(testResults.mSublibRunProp).isEqualTo("true");
        assertThat(testResults.mApkContentsPath).isEqualTo("/mnt/apk");
        assertThat(testResults.mEncryptedStoragePath).isEqualTo("");
        assertThat(testResults.mInstanceSecret).hasLength(32);
    }

    @Test
    public void createTwice() throws Exception {
        VirtualMachineManager vmm = getVirtualMachineManager();
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        String name = "test_vm_createTwice";
        deleteVirtualMachineIfExists(name);
        try (VirtualMachine vm = vmm.create(name, config)) {
            assertThrowsVmException(
                    () -> vmm.create(name, config),
                    VirtualMachineException.CODE_NAME_ALREADY_EXISTS,
                    null);
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void createAndConnectToVm() throws Exception {
        createAndConnectToVmHelper(CPU_TOPOLOGY_ONE_CPU, /* shouldUseHugepages= */ false);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void createAndConnectToVm_HostCpuTopology() throws Exception {
        createAndConnectToVmHelper(CPU_TOPOLOGY_MATCH_HOST, /* shouldUseHugepages= */ false);
    }

    @Test
    @RequiresFlagsEnabled(Flags.FLAG_PROMOTE_SET_SHOULD_USE_HUGEPAGES_TO_SYSTEM_API)
    public void createAndConnectToVm_WithHugepages() throws Exception {
        // Note: setting shouldUseHugepages to true only hints that VM wants to use transparent huge
        // pages. Whether it will actually be used depends on the value in the
        // /sys/kernel/mm/transparent_hugepages/shmem_enabled.
        // See packages/modules/Virtualization/docs/hugepages.md
        createAndConnectToVmHelper(CPU_TOPOLOGY_ONE_CPU, /* shouldUseHugepages= */ true);
    }

    @Test
    @RequiresFlagsEnabled(Flags.FLAG_PROMOTE_SET_SHOULD_USE_HUGEPAGES_TO_SYSTEM_API)
    public void createAndConnectToVm_HostCpuTopology_WithHugepages() throws Exception {
        // Note: setting shouldUseHugepages to true only hints that VM wants to use transparent huge
        // pages. Whether it will actually be used depends on the value in the
        // /sys/kernel/mm/transparent_hugepages/shmem_enabled.
        // See packages/modules/Virtualization/docs/hugepages.md
        createAndConnectToVmHelper(CPU_TOPOLOGY_MATCH_HOST, /* shouldUseHugepages= */ true);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @VsrTest(requirements = {"VSR-7.1-001.006"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.005"})
    public void vmAttestationWhenRemoteAttestationIsNotSupported() throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        assume().withMessage(
                        "This test does not apply to a device that supports Remote Attestation")
                .that(isRemoteAttestationSupported())
                .isFalse();
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary(VM_ATTESTATION_PAYLOAD_PATH)
                        .setProtectedVm(mProtectedVm)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm =
                forceCreateNewVirtualMachine("cts_attestation_with_rkpd_unsupported", config);
        byte[] challenge = new byte[32];
        Arrays.fill(challenge, (byte) 0xcc);

        // Act.
        SigningResult signingResult =
                runVmAttestationService(TAG, vm, challenge, VM_ATTESTATION_MESSAGE.getBytes());

        // Assert.
        assertThat(signingResult.status).isEqualTo(AttestationStatus.ERROR_UNSUPPORTED);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @VsrTest(requirements = {"VSR-7.1-001.006"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.005"})
    public void vmAttestationWithVendorPartitionWhenSupported() throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        assume().withMessage("Test needs Remote Attestation support")
                .that(isRemoteAttestationSupported())
                .isTrue();
        File vendorDiskImage = new File("/vendor/etc/avf/microdroid/microdroid_vendor.img");
        assumeTrue("Microdroid vendor image doesn't exist, skip", vendorDiskImage.exists());
        VirtualMachineConfig config =
                buildVmConfigWithVendor(vendorDiskImage, VM_ATTESTATION_PAYLOAD_PATH);
        VirtualMachine vm =
                forceCreateNewVirtualMachine("cts_attestation_with_vendor_module", config);
        checkVmAttestationWithValidChallenge(vm);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @VsrTest(requirements = {"VSR-7.1-001.006"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.005"})
    public void vmAttestationWhenRemoteAttestationIsSupported() throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        ensureVmAttestationSupported();
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary(VM_ATTESTATION_PAYLOAD_PATH)
                        .setProtectedVm(mProtectedVm)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm =
                forceCreateNewVirtualMachine("cts_attestation_with_rkpd_supported", config);

        // Check with an invalid challenge.
        byte[] invalidChallenge = new byte[65];
        Arrays.fill(invalidChallenge, (byte) 0xbb);
        SigningResult signingResultInvalidChallenge =
                runVmAttestationService(
                        TAG, vm, invalidChallenge, VM_ATTESTATION_MESSAGE.getBytes());
        assertThat(signingResultInvalidChallenge.status)
                .isEqualTo(AttestationStatus.ERROR_INVALID_CHALLENGE);

        // Check with a valid challenge.
        checkVmAttestationWithValidChallenge(vm);
    }

    private void checkVmAttestationWithValidChallenge(VirtualMachine vm) throws Exception {
        byte[] challenge = new byte[32];
        Arrays.fill(challenge, (byte) 0xac);
        SigningResult signingResult =
                runVmAttestationService(TAG, vm, challenge, VM_ATTESTATION_MESSAGE.getBytes());
        assertWithMessage(
                        "VM attestation should either succeed or fail when the network is unstable")
                .that(signingResult.status)
                .isAnyOf(AttestationStatus.OK, AttestationStatus.ERROR_ATTESTATION_FAILED);
        if (signingResult.status == AttestationStatus.OK) {
            X509Certificate[] certs =
                    X509Utils.validateAndParseX509CertChain(signingResult.certificateChain);
            boolean isAdvMultiTenancyEnabled =
                    isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY");
            X509Utils.verifyAvfRelatedCerts(
                    certs,
                    challenge,
                    TEST_APP_PACKAGE_NAME,
                    new String[] {},
                    isAdvMultiTenancyEnabled);
            X509Utils.verifySignature(
                    certs[0], VM_ATTESTATION_MESSAGE.getBytes(), signingResult.signature);
        }
    }

    private SigningResult attestation_signing_result(byte[] challenge) throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        ensureVmAttestationSupported();
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_tenant_attestation.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm =
                forceCreateNewVirtualMachine("cts_attestation_with_multitenant_payload", config);

        SigningResult signingResult =
                runVmAttestationService(TAG, vm, challenge, VM_ATTESTATION_MESSAGE.getBytes());
        assertWithMessage(
                        "VM attestation should either succeed or fail when the network is unstable")
                .that(signingResult.status)
                .isAnyOf(AttestationStatus.OK, AttestationStatus.ERROR_ATTESTATION_FAILED);
        return signingResult;
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @VsrTest(requirements = {"VSR-7.1-001.006"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.005"})
    public void vmAttestationWithMultipleTenantsWhenRemoteAttestationIsNotSupported()
            throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        assume().withMessage(
                        "This test does not apply to a device that supports Remote Attestation")
                .that(isRemoteAttestationSupported())
                .isFalse();
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_tenant_attestation.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm =
                forceCreateNewVirtualMachine(
                        "cts_attestation_not_supported_with_multitenant_payload", config);
        byte[] challenge = new byte[32];
        Arrays.fill(challenge, (byte) 0xcc);

        // Act.
        SigningResult signingResult =
                runVmAttestationService(TAG, vm, challenge, VM_ATTESTATION_MESSAGE.getBytes());

        // Assert.
        assertThat(signingResult.status).isEqualTo(AttestationStatus.ERROR_UNSUPPORTED);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @VsrTest(requirements = {"VSR-7.1-001.006"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.005"})
    public void vmAttestationWithMultipleTenantsWhenRemoteAttestationIsSupportedDeviceMaybeOffline()
            throws Exception {
        byte[] challenge = new byte[32];
        Arrays.fill(challenge, (byte) 0xac);
        attestation_signing_result(challenge);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @VsrTest(requirements = {"VSR-7.1-001.006"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.005"})
    public void
            vmAttestationWithMultipleTenantsWhenRemoteAttestationIsSupportedDeviceStableNetwork()
                    throws Exception {
        byte[] challenge = new byte[32];
        Arrays.fill(challenge, (byte) 0xac);
        SigningResult signingResult = attestation_signing_result(challenge);

        assume().withMessage(
                        "AttestationStatus is ERROR_ATTESTATION_FAILED possibly due to unstable"
                        + " network, nothing more to test")
                .that(signingResult)
                .isNotEqualTo(AttestationStatus.ERROR_ATTESTATION_FAILED);

        if (signingResult.status == AttestationStatus.OK) {
            X509Certificate[] certs =
                    X509Utils.validateAndParseX509CertChain(signingResult.certificateChain);
            boolean isAdvMultiTenancyEnabled =
                    isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY");
            X509Utils.verifyAvfRelatedCerts(
                    certs,
                    challenge,
                    TEST_APP_PACKAGE_NAME,
                    new String[] {TEST_TENANT_APK_NAME},
                    isAdvMultiTenancyEnabled);
            X509Utils.verifySignature(
                    certs[0], VM_ATTESTATION_MESSAGE.getBytes(), signingResult.signature);
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void createAndRunNoDebugVm() throws Exception {
        assumeSupportedDevice();

        // For most of our tests we use a debug VM so failures can be diagnosed.
        // But we do need non-debug VMs to work, so run one.
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .setVmOutputCaptured(false)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);

        TestResults testResults =
                runVmTestService(TAG, vm, (ts, tr) -> tr.mAddInteger = ts.addInteger(37, 73));
        testResults.assertNoException();
        assertThat(testResults.mAddInteger).isEqualTo(37 + 73);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void autoCloseVm() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        try (VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config)) {
            assertThat(vm.getStatus()).isEqualTo(STATUS_STOPPED);
            // close() implicitly called on stopped VM.
        }

        try (VirtualMachine vm = getVirtualMachineManager().get("test_vm")) {
            vm.run();
            assertThat(vm.getStatus()).isEqualTo(STATUS_RUNNING);
            // close() implicitly called on running VM.
        }

        try (VirtualMachine vm = getVirtualMachineManager().get("test_vm")) {
            assertThat(vm.getStatus()).isEqualTo(STATUS_STOPPED);
            getVirtualMachineManager().delete("test_vm");
            assertThat(vm.getStatus()).isEqualTo(STATUS_DELETED);
            // close() implicitly called on deleted VM.
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void autoCloseVmDescriptor() throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachineDescriptor descriptor;
        try (VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config)) {
            descriptor = vm.toDescriptor();
        }

        Parcel parcel = Parcel.obtain();
        try (descriptor) {
            // It should be ok to use at this point
            descriptor.writeToParcel(parcel, 0);
        }

        // But not now - it's been closed.
        assertThrows(IllegalStateException.class, () -> descriptor.writeToParcel(parcel, 0));
        assertThrows(
                IllegalStateException.class,
                () -> getVirtualMachineManager().importFromDescriptor("imported_vm", descriptor));

        // Closing again is fine.
        descriptor.close();

        // Tidy up
        parcel.recycle();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void vmDescriptorClosedOnImport() throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachineDescriptor descriptor;
        try (VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config)) {
            descriptor = vm.toDescriptor();
        }

        getVirtualMachineManager().importFromDescriptor("imported_vm", descriptor);
        try {
            // Descriptor has been implicitly closed
            assertThrows(
                    IllegalStateException.class,
                    () ->
                            getVirtualMachineManager()
                                    .importFromDescriptor("imported_vm2", descriptor));
        } finally {
            getVirtualMachineManager().delete("imported_vm");
        }
    }

    @Test
    public void importVmDescriptorForExistingVm() throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachineDescriptor descriptor;
        String name = "test_vm";
        try (VirtualMachine vm = forceCreateNewVirtualMachine(name, config)) {
            descriptor = vm.toDescriptor();

            assertThrowsVmException(
                    () -> getVirtualMachineManager().importFromDescriptor(name, descriptor),
                    VirtualMachineException.CODE_NAME_ALREADY_EXISTS,
                    null);
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void preconnectedBinderException() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        assertThrows(
                RuntimeException.class,
                () -> {
                    vm.binderFromPreconnectedClient(
                            () -> {
                                throw new RuntimeException(); /* oops! */
                            });
                });
    }

    @CddTest(requirements = {"3.1/C-0-1"})
    public void vmLifecycleChecks() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        assertThat(vm.getStatus()).isEqualTo(STATUS_STOPPED);

        // These methods require a running VM
        assertThrowsVmException(
                () -> vm.connectVsock(VirtualMachine.MIN_VSOCK_PORT),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_STOPPED,
                "not in running state");
        assertThrowsVmException(
                () -> vm.connectToVsockServer(VirtualMachine.MIN_VSOCK_PORT),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_STOPPED,
                "not in running state");

        vm.run();
        assertThat(vm.getStatus()).isEqualTo(STATUS_RUNNING);

        // These methods require a stopped VM
        assertThrowsVmException(
                () -> vm.run(),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_RUNNING,
                "not in stopped state");
        assertThrowsVmException(
                () -> vm.setConfig(config),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_RUNNING,
                "not in stopped state");
        assertThrowsVmException(
                () -> vm.toDescriptor(),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_RUNNING,
                "not in stopped state");
        assertThrowsVmException(
                () -> getVirtualMachineManager().delete("test_vm"),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_RUNNING,
                "not in stopped state");

        vm.stop();

        assertThat(vm.getStatus()).isEqualTo(STATUS_STOPPED);

        getVirtualMachineManager().delete("test_vm");
        assertThat(vm.getStatus()).isEqualTo(STATUS_DELETED);

        // None of these should work for a deleted VM
        assertThrowsVmException(
                () -> vm.connectVsock(VirtualMachine.MIN_VSOCK_PORT),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_DELETED,
                "deleted");
        assertThrowsVmException(
                () -> vm.connectToVsockServer(VirtualMachine.MIN_VSOCK_PORT),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_DELETED,
                "deleted");
        assertThrowsVmException(
                () -> vm.run(), VirtualMachineException.CODE_VIRTUAL_MACHINE_DELETED, "deleted");
        assertThrowsVmException(
                () -> vm.setConfig(config),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_DELETED,
                "deleted");
        assertThrowsVmException(
                () -> vm.toDescriptor(),
                VirtualMachineException.CODE_VIRTUAL_MACHINE_DELETED,
                "deleted");
        // This is indistinguishable from the VM having never existed, so the message
        // is non-specific.
        assertThrowsVmException(() -> getVirtualMachineManager().delete("test_vm"));
    }

    private static class SimpleVirtualMachineCallback implements VirtualMachineCallback {
        public final CountDownLatch started = new CountDownLatch(1);
        public final CountDownLatch stopped = new CountDownLatch(1);

        @Override
        public void onPayloadStarted(VirtualMachine vm) {
            started.countDown();
        }

        @Override
        public void onPayloadReady(VirtualMachine vm) {}

        @Override
        public void onPayloadFinished(VirtualMachine vm, int exitCode) {}

        @Override
        public void onError(VirtualMachine vm, int errorCode, String message) {}

        @Override
        public void onStopped(VirtualMachine vm, int reason) {
            stopped.countDown();
        }
    }

    private void testCrashInternal(String vmName, String killProcessName) throws Exception {
        assumeSupportedDevice();

        // kill() requires debuggable build
        assumeDebuggableBuild();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        try (VirtualMachine vm = forceCreateNewVirtualMachine(vmName, config)) {
            assertThat(vm.getStatus()).isEqualTo(STATUS_STOPPED);

            SimpleVirtualMachineCallback beforeCrash = new SimpleVirtualMachineCallback();
            vm.setCallback(Executors.newSingleThreadExecutor(), beforeCrash);

            vm.run();
            assertThat(vm.getStatus()).isEqualTo(STATUS_RUNNING);

            // Let globalTimeout to handle timeout.
            beforeCrash.started.await();

            kill(TAG, killProcessName);

            // Let globalTimeout to handle timeout.
            beforeCrash.stopped.await();
            assertThat(vm.getStatus()).isEqualTo(STATUS_STOPPED);
            assertThrowsVmException(
                    () -> vm.stop(),
                    VirtualMachineException.CODE_VIRTUAL_MACHINE_STOPPED,
                    "not running");

            // Try run again. It should recover virtmgr and run VM.
            SimpleVirtualMachineCallback afterCrash = new SimpleVirtualMachineCallback();
            vm.setCallback(Executors.newSingleThreadExecutor(), afterCrash);
            vm.run();

            // Let globalTimeout to handle timeout.
            afterCrash.started.await();
        }
    }

    @Test
    public void vm_crosvmCrash() throws Exception {
        String vmConfigName = "test_vm_crosvmCrash";
        testCrashInternal(vmConfigName, "crosvm_" + vmConfigName);
    }

    @Test
    public void vm_virtmgrCrash() throws Exception {
        testCrashInternal("test_vm_virtmgrCrash", "virtmgr_microdroid.test");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void connectVsock() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_vsock", config);

        AtomicReference<String> response = new AtomicReference<>();
        String request = "Look not into the abyss";

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (service, results) -> {
                            service.runEchoReverseServer();

                            ParcelFileDescriptor pfd =
                                    vm.connectVsock(ITestService.ECHO_REVERSE_PORT);
                            try (InputStream input = new AutoCloseInputStream(pfd);
                                    OutputStream output = new AutoCloseOutputStream(pfd)) {
                                BufferedReader reader =
                                        new BufferedReader(new InputStreamReader(input));
                                Writer writer = new OutputStreamWriter(output);
                                writer.write(request + "\n");
                                writer.flush();
                                response.set(reader.readLine());
                            }
                        });
        testResults.assertNoException();
        assertThat(response.get()).isEqualTo(new StringBuilder(request).reverse().toString());
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void binderCallbacksWork() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);

        String request = "Hello";
        CompletableFuture<String> response = new CompletableFuture<>();

        IAppCallback appCallback =
                new IAppCallback.Stub() {
                    @Override
                    public void setVmCallback(IVmCallback vmCallback) {
                        // Do this on a separate thread to simulate an asynchronous trigger,
                        // and to make sure it doesn't happen in the context of an inbound binder
                        // call.
                        new Thread() {
                            @Override
                            public void run() {
                                try {
                                    vmCallback.echoMessage(request);
                                } catch (Exception e) {
                                    response.completeExceptionally(e);
                                }
                            }
                        }.start();
                    }

                    @Override
                    public void onEchoRequestReceived(String message) {
                        response.complete(message);
                    }
                };

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (service, results) -> {
                            service.requestCallback(appCallback);
                            response.get(10, TimeUnit.SECONDS);
                        });
        testResults.assertNoException();
        assertThat(response.getNow("no response")).isEqualTo("Received: " + request);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void vmConfigGetAndSetTests() {
        // Minimal has as little as specified as possible; everything that can be is defaulted.
        VirtualMachineConfig.Builder minimalBuilder =
                new VirtualMachineConfig.Builder(getContext())
                        .setPayloadConfigPath("config/path")
                        .setProtectedVm(isProtectedVm());
        VirtualMachineConfig minimal = minimalBuilder.build();

        assertThat(minimal.getApkPath()).isNull();
        assertThat(minimal.getExtraApks()).isEmpty();
        assertThat(minimal.getDebugLevel()).isEqualTo(DEBUG_LEVEL_NONE);
        assertThat(minimal.getMemoryBytes()).isEqualTo(0);
        assertThat(minimal.getCpuTopology()).isEqualTo(CPU_TOPOLOGY_ONE_CPU);
        assertThat(minimal.getPayloadBinaryName()).isNull();
        assertThat(minimal.getPayloadConfigPath()).isEqualTo("config/path");
        assertThat(minimal.isProtectedVm()).isEqualTo(isProtectedVm());
        assertThat(minimal.isEncryptedStorageEnabled()).isFalse();
        assertThat(minimal.getEncryptedStorageBytes()).isEqualTo(0);
        assertThat(minimal.isVmOutputCaptured()).isFalse();
        assertThat(minimal.getOs()).isEqualTo("microdroid");
        if (promoteSetShouldUseHugepagesToSystemApi()) {
            assertThat(minimal.shouldUseHugepages()).isFalse();
        }

        // Maximal has everything that can be set to some non-default value. (And has different
        // values than minimal for the required fields.)
        VirtualMachineConfig.Builder maximalBuilder =
                new VirtualMachineConfig.Builder(getContext())
                        .setProtectedVm(mProtectedVm)
                        .setPayloadBinaryName("binary.so")
                        .setApkPath("/apk/path")
                        .addExtraApk("package.name1")
                        .addExtraApk("package.name2")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setMemoryBytes(42)
                        .setCpuTopology(CPU_TOPOLOGY_MATCH_HOST)
                        .setEncryptedStorageBytes(1_000_000)
                        .setVmOutputCaptured(true)
                        .setOs("microdroid_gki-android14-6.1");
        if (promoteSetShouldUseHugepagesToSystemApi()) {
            maximalBuilder.setShouldUseHugepages(true);
        }
        VirtualMachineConfig maximal = maximalBuilder.build();

        assertThat(maximal.getApkPath()).isEqualTo("/apk/path");
        assertThat(maximal.getExtraApks())
                .containsExactly("package.name1", "package.name2")
                .inOrder();
        assertThat(maximal.getDebugLevel()).isEqualTo(DEBUG_LEVEL_FULL);
        assertThat(maximal.getMemoryBytes()).isEqualTo(42);
        assertThat(maximal.getCpuTopology()).isEqualTo(CPU_TOPOLOGY_MATCH_HOST);
        assertThat(maximal.getPayloadBinaryName()).isEqualTo("binary.so");
        assertThat(maximal.getPayloadConfigPath()).isNull();
        assertThat(maximal.isProtectedVm()).isEqualTo(isProtectedVm());
        assertThat(maximal.isEncryptedStorageEnabled()).isTrue();
        assertThat(maximal.getEncryptedStorageBytes()).isEqualTo(1_000_000);
        assertThat(maximal.isVmOutputCaptured()).isTrue();
        assertThat(maximal.getOs()).isEqualTo("microdroid_gki-android14-6.1");
        if (promoteSetShouldUseHugepagesToSystemApi()) {
            assertThat(maximal.shouldUseHugepages()).isTrue();
        }

        assertThat(minimal.isCompatibleWith(maximal)).isFalse();
        assertThat(minimal.isCompatibleWith(minimal)).isTrue();
        assertThat(maximal.isCompatibleWith(maximal)).isTrue();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void vmConfigBuilderValidationTests() {
        VirtualMachineConfig.Builder builder =
                new VirtualMachineConfig.Builder(getContext()).setProtectedVm(mProtectedVm);

        // All your null are belong to me.
        assertThrows(NullPointerException.class, () -> new VirtualMachineConfig.Builder(null));
        assertThrows(NullPointerException.class, () -> builder.setApkPath(null));
        assertThrows(NullPointerException.class, () -> builder.addExtraApk(null));
        assertThrows(NullPointerException.class, () -> builder.setPayloadConfigPath(null));
        assertThrows(NullPointerException.class, () -> builder.setPayloadBinaryName(null));
        assertThrows(NullPointerException.class, () -> builder.setVendorDiskImage(null));
        assertThrows(NullPointerException.class, () -> builder.setOs(null));

        // Individual property checks.
        assertThrows(
                IllegalArgumentException.class, () -> builder.setApkPath("relative/path/to.apk"));
        assertThrows(
                IllegalArgumentException.class, () -> builder.setPayloadBinaryName("dir/file.so"));
        assertThrows(IllegalArgumentException.class, () -> builder.setDebugLevel(-1));
        assertThrows(IllegalArgumentException.class, () -> builder.setMemoryBytes(0));
        assertThrows(IllegalArgumentException.class, () -> builder.setCpuTopology(-1));
        assertThrows(IllegalArgumentException.class, () -> builder.setEncryptedStorageBytes(0));

        // Consistency checks enforced at build time.
        Exception e;
        e = assertThrows(IllegalStateException.class, () -> builder.build());
        assertThat(e).hasMessageThat().contains("setPayloadBinaryName must be called");

        VirtualMachineConfig.Builder protectedNotSet =
                new VirtualMachineConfig.Builder(getContext()).setPayloadBinaryName("binary.so");
        e = assertThrows(IllegalStateException.class, () -> protectedNotSet.build());
        assertThat(e).hasMessageThat().contains("setProtectedVm must be called");

        VirtualMachineConfig.Builder captureInputOnNonDebuggable =
                newVmConfigBuilderWithPayloadBinary("binary.so")
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_NONE)
                        .setVmConsoleInputSupported(true);
        e = assertThrows(IllegalStateException.class, () -> captureInputOnNonDebuggable.build());
        assertThat(e).hasMessageThat().contains("debug level must be FULL to use console input");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void compatibleConfigTests() {
        VirtualMachineConfig baseline = newBaselineBuilder().build();

        // A config must be compatible with itself
        assertConfigCompatible(baseline, newBaselineBuilder()).isTrue();

        // Changes that must always be compatible
        assertConfigCompatible(baseline, newBaselineBuilder().setMemoryBytes(99)).isTrue();
        assertConfigCompatible(
                        baseline, newBaselineBuilder().setCpuTopology(CPU_TOPOLOGY_MATCH_HOST))
                .isTrue();
        if (promoteSetShouldUseHugepagesToSystemApi()) {
            assertConfigCompatible(baseline, newBaselineBuilder().setShouldUseHugepages(true))
                    .isTrue();
        }

        // Changes that must be incompatible, since they must change the VM identity.
        assertConfigCompatible(baseline, newBaselineBuilder().addExtraApk("foo")).isFalse();
        assertConfigCompatible(baseline, newBaselineBuilder().setDebugLevel(DEBUG_LEVEL_FULL))
                .isFalse();
        assertConfigCompatible(baseline, newBaselineBuilder().setPayloadBinaryName("different"))
                .isFalse();
        assertConfigCompatible(
                        baseline, newBaselineBuilder().setVendorDiskImage(new File("/foo/bar")))
                .isFalse();
        int capabilities = getVirtualMachineManager().getCapabilities();
        if ((capabilities & CAPABILITY_PROTECTED_VM) != 0
                && (capabilities & CAPABILITY_NON_PROTECTED_VM) != 0) {
            assertConfigCompatible(baseline, newBaselineBuilder().setProtectedVm(!isProtectedVm()))
                    .isFalse();
        }

        // Changes that were incompatible but are currently compatible, but not guaranteed to be
        // so in the API spec.
        assertConfigCompatible(baseline, newBaselineBuilder().setApkPath("/different")).isTrue();

        VirtualMachineConfig.Builder debuggableBuilder =
                newBaselineBuilder().setDebugLevel(DEBUG_LEVEL_FULL);
        VirtualMachineConfig debuggable = debuggableBuilder.build();
        assertConfigCompatible(debuggable, debuggableBuilder.setVmOutputCaptured(true)).isFalse();
        assertConfigCompatible(debuggable, debuggableBuilder.setVmOutputCaptured(false)).isTrue();
        assertConfigCompatible(debuggable, debuggableBuilder.setVmConsoleInputSupported(true))
                .isFalse();

        VirtualMachineConfig currentContextConfig =
                new VirtualMachineConfig.Builder(getContext())
                        .setProtectedVm(isProtectedVm())
                        .setPayloadBinaryName("binary.so")
                        .build();

        // packageName is not directly exposed by the config, so we have to be a bit creative
        // to modify it.
        Context otherContext =
                new ContextWrapper(getContext()) {
                    @Override
                    public String getPackageName() {
                        return "other.package.name";
                    }
                };
        VirtualMachineConfig.Builder otherContextBuilder =
                new VirtualMachineConfig.Builder(otherContext)
                        .setProtectedVm(isProtectedVm())
                        .setPayloadBinaryName("binary.so");
        assertConfigCompatible(currentContextConfig, otherContextBuilder).isFalse();

        VirtualMachineConfig microdroidOsConfig = newBaselineBuilder().setOs("microdroid").build();
        VirtualMachineConfig.Builder otherOsBuilder =
                newBaselineBuilder().setOs("microdroid_gki-android14-6.1");
        assertConfigCompatible(microdroidOsConfig, otherOsBuilder).isFalse();
    }

    private VirtualMachineConfig.Builder newBaselineBuilder() {
        return newVmConfigBuilderWithPayloadBinary("binary.so").setApkPath("/apk/path");
    }

    private BooleanSubject assertConfigCompatible(
            VirtualMachineConfig baseline, VirtualMachineConfig.Builder builder) {
        return assertThat(builder.build().isCompatibleWith(baseline));
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void vmUnitTests() throws Exception {
        VirtualMachineConfig.Builder builder = newVmConfigBuilderWithPayloadBinary("binary.so");
        VirtualMachineConfig config = builder.build();
        VirtualMachine vm = forceCreateNewVirtualMachine("vm_name", config);

        assertThat(vm.getName()).isEqualTo("vm_name");
        assertThat(vm.getConfig().getPayloadBinaryName()).isEqualTo("binary.so");
        assertThat(vm.getConfig().getMemoryBytes()).isEqualTo(0);

        VirtualMachineConfig compatibleConfig = builder.setMemoryBytes(42).build();
        vm.setConfig(compatibleConfig);

        assertThat(vm.getName()).isEqualTo("vm_name");
        assertThat(vm.getConfig().getPayloadBinaryName()).isEqualTo("binary.so");
        assertThat(vm.getConfig().getMemoryBytes()).isEqualTo(42);

        assertThat(getVirtualMachineManager().get("vm_name")).isSameInstanceAs(vm);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void testAvfRequiresUpdatableApex() throws Exception {
        assertWithMessage("Devices that support AVF must also support updatable APEX")
                .that(SystemProperties.getBoolean("ro.apex.updatable", false))
                .isTrue();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void vmmGetAndCreate() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        VirtualMachineManager vmm = getVirtualMachineManager();
        String vmName = "vmName";

        try {
            // VM does not yet exist
            assertThat(vmm.get(vmName)).isNull();

            VirtualMachine vm1 = vmm.create(vmName, config);

            // Now it does, and we should get the same instance back
            assertThat(vmm.get(vmName)).isSameInstanceAs(vm1);
            assertThat(vmm.getOrCreate(vmName, config)).isSameInstanceAs(vm1);

            // Can't recreate it though
            assertThrowsVmException(() -> vmm.create(vmName, config));

            vmm.delete(vmName);
            assertThat(vmm.get(vmName)).isNull();

            // Now that we deleted the old one, this should create rather than get, and it should be
            // a new instance.
            VirtualMachine vm2 = vmm.getOrCreate(vmName, config);
            assertThat(vm2).isNotSameInstanceAs(vm1);

            // The old one must remain deleted, or we'd have two VirtualMachine instances referring
            // to the same VM.
            assertThat(vm1.getStatus()).isEqualTo(STATUS_DELETED);

            // Subsequent gets should return this new one.
            assertThat(vmm.get(vmName)).isSameInstanceAs(vm2);
            assertThat(vmm.getOrCreate(vmName, config)).isSameInstanceAs(vm2);
        } finally {
            vmm.delete(vmName);
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void vmFilesStoredInDeDirWhenCreatedFromDEContext() throws Exception {
        final Context ctx = getContext().createDeviceProtectedStorageContext();
        final int userId = ctx.getUserId();
        final VirtualMachineManager vmm = ctx.getSystemService(VirtualMachineManager.class);
        VirtualMachineConfig config = newVmConfigBuilderWithPayloadBinary("binary.so").build();
        try {
            VirtualMachine vm = vmm.create("vm-name", config);
            // TODO(b/261430346): what about non-primary user?
            assertThat(vm.getRootDir().getAbsolutePath())
                    .isEqualTo(
                            "/data/user_de/" + userId + "/com.android.microdroid.test/vm/vm-name");
        } finally {
            vmm.delete("vm-name");
        }
    }

    // b/441586847 - There was once a bug where the VM dir was malformed for DE
    // context only. Check for a regression by forcing the VM to be loaded from
    // disk.
    @Test
    public void loadVmFilesStoredInDeDir() throws Exception {
        final Context ctx = getContext().createDeviceProtectedStorageContext();
        final VirtualMachineManager vmm = ctx.getSystemService(VirtualMachineManager.class);
        VirtualMachineConfig config = newVmConfigBuilderWithPayloadBinary("binary.so").build();
        try {
            VirtualMachine vm = vmm.create("vm-name", config);
            vm.close();
            vmm.testOnlyClearCache();
            assertThat(vmm.get("vm-name")).isNotNull();
        } finally {
            vmm.delete("vm-name");
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void vmFilesStoredInCeDirWhenCreatedFromCEContext() throws Exception {
        final Context ctx = getContext().createCredentialProtectedStorageContext();
        final int userId = ctx.getUserId();
        final VirtualMachineManager vmm = ctx.getSystemService(VirtualMachineManager.class);
        VirtualMachineConfig config = newVmConfigBuilderWithPayloadBinary("binary.so").build();
        try {
            VirtualMachine vm = vmm.create("vm-name", config);
            // TODO(b/261430346): what about non-primary user?
            assertThat(vm.getRootDir().getAbsolutePath())
                    .isEqualTo("/data/user/" + userId + "/com.android.microdroid.test/vm/vm-name");
        } finally {
            vmm.delete("vm-name");
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void differentManagersForDifferentContexts() throws Exception {
        final Context ceCtx = getContext().createCredentialProtectedStorageContext();
        final Context deCtx = getContext().createDeviceProtectedStorageContext();
        assertThat(ceCtx.getSystemService(VirtualMachineManager.class))
                .isNotSameInstanceAs(deCtx.getSystemService(VirtualMachineManager.class));
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void createVmWithConfigRequiresPermission() throws Exception {
        assumeSupportedDevice();
        revokePermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config.json")
                        .setMemoryBytes(minMemoryRequired())
                        .build();

        VirtualMachine vm =
                forceCreateNewVirtualMachine("test_vm_config_requires_permission", config);

        SecurityException e =
                assertThrows(
                        SecurityException.class, () -> runVmTestService(TAG, vm, (ts, tr) -> {}));
        assertThat(e)
                .hasMessageThat()
                .contains("android.permission.USE_CUSTOM_VIRTUAL_MACHINE permission");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void deleteVm() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_delete", config);
        VirtualMachineManager vmm = getVirtualMachineManager();
        vmm.delete("test_vm_delete");

        // VM should no longer exist
        assertThat(vmm.get("test_vm_delete")).isNull();

        // Can't start the VM even with an existing reference
        assertThrowsVmException(vm::run);

        // Can't delete the VM since it no longer exists
        assertThrowsVmException(() -> vmm.delete("test_vm_delete"));
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void deleteVmFiles() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidExitNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_delete", config);
        vm.run();
        // If we explicitly stop a VM, that triggers some tidy up; so for this test we start a VM
        // that immediately stops itself.
        while (vm.getStatus() == STATUS_RUNNING) {
            Thread.sleep(100);
        }

        // Delete the files without telling VMM. This isn't a good idea, but we can't stop an
        // app doing it, and we should recover from it.
        for (File f : vm.getRootDir().listFiles()) {
            Files.delete(f.toPath());
        }
        vm.getRootDir().delete();

        VirtualMachineManager vmm = getVirtualMachineManager();
        assertThat(vmm.get("test_vm_delete")).isNull();
        assertThat(vm.getStatus()).isEqualTo(STATUS_DELETED);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void validApkPathIsAccepted() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setApkPath(getContext().getPackageCodePath())
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_explicit_apk_path", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mApkContentsPath = ts.getApkContentsPath();
                        });
        testResults.assertNoException();
        assertThat(testResults.mApkContentsPath).isEqualTo("/mnt/apk");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void invalidVmNameIsRejected() {
        VirtualMachineManager vmm = getVirtualMachineManager();
        assertThrows(IllegalArgumentException.class, () -> vmm.get("../foo"));
        assertThrows(IllegalArgumentException.class, () -> vmm.get(".."));
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void extraApk() throws Exception {
        assumeSupportedDevice();

        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_extra_apk.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_extra_apk", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mExtraApkTestProp =
                                    ts.readProperty(
                                            "debug.microdroid.test.extra_apk_build_manifest");
                        });
        assertThat(testResults.mExtraApkTestProp).isEqualTo("PASS");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void multipleTenantServices() throws Exception {
        assumeSupportedDevice();
        // TODO(b/465728787): The signing keys used for virt apex in Cuttlefish do not match
        // the expected public keys in TenancyConfig (vm_config_test_multi_tenants.json).
        // Disable the test on CF till we implement robust strategy for testing different
        // signing configuration of DUT.
        assume().withMessage("Skip on CF, which has virt apex signed with different key")
                .that(isCuttlefish())
                .isFalse();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);

        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_test_multi_tenants.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_tenant_services", config);
        CompletableFuture<String> prop = new CompletableFuture<>();
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Integer> exitCodeFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        try {
                            ITestService tsOnAPort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            String val =
                                    tsOnAPort.readProperty(
                                            "debug.microdroid.test.tenant_packages_mounted");
                            prop.complete(val);
                            // Connect to the second service!
                            ITestService tsOnAlternatePort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.ALTERNATE_PORT));
                            String valFromAnotherTenant =
                                    tsOnAlternatePort.readProperty(
                                            "debug.microdroid.test.tenant_packages_mounted");
                            assertWithMessage("Received different values from different tenants")
                                    .that(valFromAnotherTenant)
                                    .isEqualTo(val);
                            tsOnAPort.quit();
                            tsOnAlternatePort.quit();
                        } catch (Exception e) {
                            exception.complete(e);
                        }
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        exitCodeFuture.complete(exitCode);
                    }
                };
        listener.runToFinish(TAG, vm);
        assertWithMessage(
                        "Unexpected exception while running test_vm_tenant_services's"
                            + " onPayloadReady callback")
                .that(exception.getNow(null))
                .isNull();

        assertWithMessage("debug.microdroid.test.tenant_packages_mounted != PASS")
                .that(prop.getNow(null))
                .isEqualTo("PASS");
        assertThat(exitCodeFuture.getNow(500)).isEqualTo(0);
    }

    @Test
    @CddTest
    public void multipleTenantUids() throws Exception {
        assumeSupportedDevice();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_test_multi_tenants.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        final int NUMBER_OF_TENANTS_IN_CONFIG = 3;
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_tenant_uids", config);
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Integer> exitCodeFuture = new CompletableFuture<>();

        CompletableFuture<Integer> tenant1UidFuture = new CompletableFuture<>();
        CompletableFuture<Integer> tenant2UidFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        try {
                            ITestService tenant1Service =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            tenant1UidFuture.complete(tenant1Service.getUid());
                            ITestService tenant2Service =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.ALTERNATE_PORT));
                            tenant2UidFuture.complete(tenant2Service.getUid());

                            tenant1Service.quit();
                            tenant2Service.quit();
                        } catch (Exception e) {
                            exception.complete(e);
                        }
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        exitCodeFuture.complete(exitCode);
                    }
                };
        listener.runToFinish(TAG, vm);
        Integer tenant1Uid = tenant1UidFuture.get();
        Integer tenant2Uid = tenant2UidFuture.get();

        assertWithMessage("Tenant UIDs should be distinct")
                .that(tenant1Uid)
                .isNotEqualTo(tenant2Uid);

        List<Integer> validUids = generateValidUidsForTenants(NUMBER_OF_TENANTS_IN_CONFIG);
        assertWithMessage("Tenant 1 UID should be one of " + validUids)
                .that(tenant1Uid)
                .isIn(validUids);
        assertWithMessage("Tenant 2 UID should be one of " + validUids)
                .that(tenant2Uid)
                .isIn(validUids);

        assertWithMessage(
                        "Unexpected exception while running test_vm_tenant_uids's"
                                + " onPayloadReady callback")
                .that(exception.getNow(null))
                .isNull();

        assertThat(exitCodeFuture.getNow(500)).isEqualTo(0);
    }

    @Test
    public void multiTenantSelinuxDomain() throws Exception {
        assumeSupportedDevice();

        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);

        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
        installApp("MicrodroidTestHelperAppAlternateTenant.apk");
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_test_multi_tenants.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("vm_selinux_domain", config);
        CompletableFuture<String> context1 = new CompletableFuture<>();
        CompletableFuture<String> context2 = new CompletableFuture<>();
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Integer> exitCodeFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        try {
                            ITestService tsOnAPort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            String domain = tsOnAPort.getselinuxdomain();
                            context1.complete(domain);
                            ITestService tsOnAlternatePort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.ALTERNATE_PORT));
                            String domain2 = tsOnAlternatePort.getselinuxdomain();
                            context2.complete(domain2);
                            tsOnAPort.quit();
                            tsOnAlternatePort.quit();
                        } catch (Exception e) {
                            exception.complete(e);
                        }
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        exitCodeFuture.complete(exitCode);
                    }
                };
        listener.runToFinish(TAG, vm);
        assertWithMessage(
                        "Unexpected exception while running vm_selinux_domain"
                                + " onPayloadReady callback")
                .that(exception.getNow(null))
                .isNull();
        assertThat(exitCodeFuture.getNow(500)).isEqualTo(0);

        // Assert that the first tenant got the expected domain (as specified in config)
        assertWithMessage("Unexpected Selinux context of the tenant")
                .that(context1.getNow(null))
                .isEqualTo("u:r:appsearch_tenant:s0");
        // Assert that the second tenant got the default/fallback context
        assertWithMessage("Second tenant is using unexpected default/fallback context")
                .that(context2.getNow(null))
                .isEqualTo("u:r:microdroid_app:s0");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void multiTenantEncryptedStoragePath() throws Exception {
        assumeSupportedDevice();
        assumeTrue("Missing Updatable VM support", isUpdatableVmSupported());

        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);

        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_test_multi_tenants.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm =
                forceCreateNewVirtualMachine("test_vm_tenant_encrypted_storage_path", config);
        CompletableFuture<String> tenant1EncryptedStoragePath = new CompletableFuture<>();
        CompletableFuture<String> tenant2EncryptedStoragePath = new CompletableFuture<>();
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Integer> exitCodeFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        try {
                            ITestService tsOnAPort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            tenant1EncryptedStoragePath.complete(
                                    tsOnAPort.getEncryptedStoragePath());

                            ITestService tsOnAlternatePort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.ALTERNATE_PORT));
                            tenant2EncryptedStoragePath.complete(
                                    tsOnAlternatePort.getEncryptedStoragePath());

                            tsOnAPort.quit();
                            tsOnAlternatePort.quit();
                        } catch (Exception e) {
                            exception.complete(e);
                        }
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        exitCodeFuture.complete(exitCode);
                    }
                };
        listener.runToFinish(TAG, vm);
        assertWithMessage(
                        "Unexpected exception while running"
                                + " test_vm_tenant_encrypted_storage_path's onPayloadReady"
                                + " callback")
                .that(exception.getNow(null))
                .isNull();

        assertWithMessage("Tenant 1 encrypted storage path should be specific to the tenant")
                .that(tenant1EncryptedStoragePath.getNow(null))
                .isEqualTo("/mnt/encryptedstore/com.android.microdroid.test");
        assertWithMessage("Tenant 2 encrypted storage path should be specific to the tenant")
                .that(tenant2EncryptedStoragePath.getNow(null))
                .isEqualTo("/mnt/encryptedstore/com.android.microdroid.test_alternate_tenant");
        assertThat(exitCodeFuture.getNow(500)).isEqualTo(0);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void addingMoretenantsIsSupported() throws Exception {
        assumeSupportedDevice();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));

        // First run with a single tenant.
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_single_tenant.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_add_tenant", config);
        CompletableFuture<String> result = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted != PASS")
                .that(result.getNow(null))
                .isEqualTo("PASS");

        // Re-run the VM with more tenants
        config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_test_multi_tenants.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        vm.setConfig(config);
        CompletableFuture<String> prop = new CompletableFuture<>();
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Integer> exitCodeFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        try {
                            ITestService tsOnAPort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            String val =
                                    tsOnAPort.readProperty(
                                            "debug.microdroid.test.tenant_packages_mounted");
                            prop.complete(val);
                            // Connect to the second service!
                            ITestService tsOnAlternatePort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.ALTERNATE_PORT));
                            String valFromAnotherTenant =
                                    tsOnAlternatePort.readProperty(
                                            "debug.microdroid.test.tenant_packages_mounted");
                            assertWithMessage("Received different values from different tenants")
                                    .that(valFromAnotherTenant)
                                    .isEqualTo(val);
                            tsOnAPort.quit();
                            tsOnAlternatePort.quit();
                        } catch (Exception e) {
                            exception.complete(e);
                        }
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        exitCodeFuture.complete(exitCode);
                    }
                };
        listener.runToFinish(TAG, vm);
        assertWithMessage(
                        "Unexpected exception while running test_vm_add_tenant's"
                                + " onPayloadReady callback")
                .that(exception.getNow(null))
                .isNull();

        assertWithMessage("debug.microdroid.test.tenant_packages_mounted != PASS")
                .that(prop.getNow(null))
                .isEqualTo("PASS");
        assertThat(exitCodeFuture.getNow(500)).isEqualTo(0);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void testEncryptedStorageIsPersistentOnAddTenant() throws Exception {
        assumeSupportedDevice();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));

        // First run with a single tenant.
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_single_tenant.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm =
                forceCreateNewVirtualMachine("test_vm_tenant_encrypted_storage_persistent", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            String encryptedStoragePath = ts.getEncryptedStoragePath();
                            tr.mEncryptedStoragePath = encryptedStoragePath;
                            ts.writeToFile(
                                    /* content= */ EXAMPLE_STRING,
                                    /* path= */ encryptedStoragePath + "/test_file");
                        });
        testResults.assertNoException();
        assertThat(testResults.mEncryptedStoragePath)
                .isEqualTo("/mnt/encryptedstore/com.android.microdroid.test");

        // Re-run the VM with more tenants
        config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_test_multi_tenants.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        vm.setConfig(config);

        // Re-run the same VM & verify the file persisted.
        CompletableFuture<String> prop = new CompletableFuture<>();
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Integer> exitCodeFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        try {
                            ITestService tsOnAPort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            ITestService tsOnAlternatePort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.ALTERNATE_PORT));
                            String encryptedStoragePath = tsOnAPort.getEncryptedStoragePath();

                            String content =
                                    tsOnAPort.readFromFile(encryptedStoragePath + "/test_file");
                            prop.complete(content);

                            tsOnAPort.quit();
                            tsOnAlternatePort.quit();
                        } catch (Exception e) {
                            exception.complete(e);
                        }
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        exitCodeFuture.complete(exitCode);
                    }
                };
        listener.runToFinish(TAG, vm);
        assertWithMessage(
                        "Unexpected exception while running"
                            + " test_vm_tenant_encrypted_storage_persistent's onPayloadReady"
                            + " callback")
                .that(exception.getNow(null))
                .isNull();

        assertWithMessage("File content should be the same as before")
                .that(prop.getNow(null))
                .isEqualTo(EXAMPLE_STRING);
        assertThat(exitCodeFuture.getNow(500)).isEqualTo(0);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void duplicateTenantsAreRejected() throws Exception {
        assumeSupportedDevice();
        assumeTrue("Missing Updatable VM support", isUpdatableVmSupported());

        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);

        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_invalid_duplicate_tenants.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_duplicate_tenants", config);
        CompletableFuture<String> res = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(res.getNow(null))
                .isNull();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void invalidTenantApkAuthority() throws Exception {
        assumeSupportedDevice();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_invalid_tenant_auth.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("tenant_with_different_cert", config);
        CompletableFuture<String> res = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(res.getNow(null))
                .isNull();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void invalidTenantApexAuthority() throws Exception {
        assumeSupportedDevice();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_invalid_tenant_apex_auth.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("tenant_with_different_cert_fail", config);
        CompletableFuture<String> res = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(res.getNow(null))
                .isNull();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void invalidTenantApexVersion() throws Exception {
        assumeSupportedDevice();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_invalid_tenant_apex_version.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("tenant_with_lower_version_fail", config);
        CompletableFuture<String> res = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(res.getNow(null))
                .isNull();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void invalidTenantRollbackIndex() throws Exception {
        assumeSupportedDevice();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
        // MicrodroidTestHelperAppRelaxedRollbackProtection_V6.apk has rollback_index:1
        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V6.apk");
        // vm_config_tenant_rollback_index.json expects min_version: 2 for package
        // com.android.microdroid.test_relaxed_rollback_protection_scheme
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_tenant_rollback_index.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("tenant_rollback_index_1", config);
        CompletableFuture<String> result_rb_1 = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(result_rb_1.getNow(null))
                .isNull();

        // MicrodroidTestHelperAppRelaxedRollbackProtection_V7*.apk has rollback_index:2
        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V7_inc_rollback_version.apk");
        VirtualMachine vm2 = forceCreateNewVirtualMachine("tenant_rollback_index_2", config);
        CompletableFuture<String> result_rb_2 = readTenantPackagesMounted(vm2);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted != PASS")
                .that(result_rb_2.getNow(null))
                .isEqualTo("PASS");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void multiTenantInstanceSpecRollbackTest() throws Exception {
        assumeSupportedDevice();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));

        // MicrodroidTestHelperAppRelaxedRollbackProtection_V7*.apk has rollback_index:2
        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V7_inc_rollback_version.apk");

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_tenant_rollback_index_instance_spec.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        // First boot - InstanceSpec should be created
        VirtualMachine vm1 =
                forceCreateNewVirtualMachine("multi_tenant_instance_spec_rollback", config);
        CompletableFuture<String> result_v2_boot1 = readTenantPackagesMounted(vm1);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted != PASS")
                .that(result_v2_boot1.getNow(null))
                .isEqualTo("PASS");

        // Second boot - InstanceSpec should be read and verified
        VirtualMachine vm2 = getVirtualMachineManager().get("multi_tenant_instance_spec_rollback");
        CompletableFuture<String> result_v2_boot2 = readTenantPackagesMounted(vm2);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted != PASS")
                .that(result_v2_boot2.getNow(null))
                .isEqualTo("PASS");

        // Install lower version - MicrodroidTestHelperAppRelaxedRollbackProtection_V6.apk has
        // rollback_index:1
        uninstallApp(RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME);

        getVirtualMachineManager().testOnlyClearCache();

        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V6.apk", "-d");

        VirtualMachine vm3 = getVirtualMachineManager().get("multi_tenant_instance_spec_rollback");
        BootResult bootResult = tryBootVm(TAG, vm3);
        assertThat(bootResult.payloadStarted).isFalse();
        assertThat(bootResult.deathReason)
                .isEqualTo(
                        VirtualMachineCallback.STOP_REASON_MICRODROID_PAYLOAD_VERIFICATION_FAILED);
    }

    private CompletableFuture<String> readTenantPackagesMounted(VirtualMachine vm)
            throws Exception {
        CompletableFuture<String> prop = new CompletableFuture<>();
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Integer> exitCodeFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        try {
                            // Note (from vm_config_apk_tenant): this service is registered by
                            // tenant task.
                            ITestService tsTenant =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            String val =
                                    tsTenant.readProperty(
                                            "debug.microdroid.test.tenant_packages_mounted");
                            prop.complete(val);
                        } catch (Exception e) {
                            exception.complete(e);
                        } finally {
                            // There maybe instances of `ITestService` running in the VM.
                            // Force stop the VM.
                            forceStop(vm);
                        }
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        exitCodeFuture.complete(exitCode);
                    }
                };
        listener.runToFinish(TAG, vm);
        assertWithMessage(
                        "Unexpected exception while running test_vm_tenant_apk's onPayloadReady"
                                + " callback")
                .that(exception.getNow(null))
                .isNull();
        return prop;
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void extraApkInVmConfig() throws Exception {
        assumeSupportedDevice();

        assumeTrue("MultiTenancy feature not supported", isFeatureMultiTenantSupported());
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .addExtraApk(VM_SHARE_APP_PACKAGE_NAME)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_extra_apk", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mExtraApkTestProp =
                                    ts.readProperty("debug.microdroid.test.extra_apk_vm_share");
                        });
        assertThat(testResults.mExtraApkTestProp).isEqualTo("PASS");
    }

    @Test
    public void bootFailsWhenLowMem() throws Exception {
        for (int memMib : new int[] {10, 20, 40}) {
            VirtualMachineConfig lowMemConfig =
                    newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                            .setMemoryBytes(memMib)
                            .setDebugLevel(DEBUG_LEVEL_NONE)
                            .setVmOutputCaptured(false)
                            .build();
            VirtualMachine vm = forceCreateNewVirtualMachine("low_mem", lowMemConfig);
            final CompletableFuture<Boolean> onPayloadReadyExecuted = new CompletableFuture<>();
            final CompletableFuture<Boolean> onStoppedExecuted = new CompletableFuture<>();
            VmEventListener listener =
                    new VmEventListener() {
                        @Override
                        public void onPayloadReady(VirtualMachine vm) {
                            onPayloadReadyExecuted.complete(true);
                            super.onPayloadReady(vm);
                        }

                        @Override
                        public void onStopped(VirtualMachine vm, int reason) {
                            onStoppedExecuted.complete(true);
                            super.onStopped(vm, reason);
                        }
                    };
            listener.runToFinish(TAG, vm);
            // Assert that onStopped() was executed but onPayloadReady() was never run
            assertThat(onStoppedExecuted.getNow(false)).isTrue();
            assertThat(onPayloadReadyExecuted.getNow(false)).isFalse();
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void changingNonDebuggableVmDebuggableInvalidatesVmIdentity() throws Exception {
        // Debuggability changes initrd which is verified by pvmfw.
        // Therefore, skip this on non-protected VM.
        assumeProtectedVM();
        changeDebugLevel(DEBUG_LEVEL_NONE, DEBUG_LEVEL_FULL);
    }

    // Copy the Vm directory, creating the target Vm directory if it does not already exist.
    private void copyVmDirectory(String sourceVmName, String targetVmName) throws IOException {
        Path sourceVm = getVmDirectory(sourceVmName);
        Path targetVm = getVmDirectory(targetVmName);
        if (!Files.exists(targetVm)) {
            Files.createDirectories(targetVm);
        }

        try (Stream<Path> stream = Files.list(sourceVm)) {
            for (Path f : stream.collect(toList())) {
                Files.copy(f, targetVm.resolve(f.getFileName()), REPLACE_EXISTING);
            }
        }
    }

    private Path getVmDirectory(String vmName) {
        Context context = getContext();
        Path filePath = Paths.get(context.getDataDir().getPath(), "vm", vmName);
        return filePath;
    }

    // Create a fresh VM with the given `vmName`, instance_id & instance.img. This function creates
    // a Vm with a different temporary name & copies it to target VM directory. This ensures this
    // VM is not in cache of `VirtualMachineManager` which makes it possible to modify underlying
    // files.
    private void createUncachedVmWithName(
            String vmName, VirtualMachineConfig config, File vmIdBackup, File vmInstanceBackup)
            throws Exception {
        deleteVirtualMachineIfExists(vmName);
        forceCreateNewVirtualMachine("test_vm_tmp", config);
        copyVmDirectory("test_vm_tmp", vmName);
        if (vmInstanceBackup != null) {
            Files.copy(
                    vmInstanceBackup.toPath(),
                    getVmFile(vmName, "instance.img").toPath(),
                    REPLACE_EXISTING);
        }
        if (vmIdBackup != null) {
            Files.copy(
                    vmIdBackup.toPath(),
                    getVmFile(vmName, "instance_id").toPath(),
                    REPLACE_EXISTING);
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void changingDebuggableVmNonDebuggableInvalidatesVmIdentity() throws Exception {
        // Debuggability changes initrd which is verified by pvmfw.
        // Therefore, skip this on non-protected VM.
        assumeProtectedVM();
        changeDebugLevel(DEBUG_LEVEL_FULL, DEBUG_LEVEL_NONE);
    }

    private void changeDebugLevel(int fromLevel, int toLevel) throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig.Builder builder =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(fromLevel)
                        .setVmOutputCaptured(false);
        VirtualMachineConfig normalConfig = builder.build();
        assertThat(tryBootVmWithConfig(normalConfig, "test_vm").payloadStarted).isTrue();

        // Try to run the VM again with the previous instance
        // We need to make sure that no changes on config don't invalidate the identity, to compare
        // the result with the below "different debug level" test.
        File vmInstanceBackup = null, vmIdBackup = null;
        File vmInstance = getVmFile("test_vm", "instance.img");
        File vmId = getVmFile("test_vm", "instance_id");
        if (vmInstance.exists()) {
            vmInstanceBackup = File.createTempFile("instance", ".img");
            Files.copy(vmInstance.toPath(), vmInstanceBackup.toPath(), REPLACE_EXISTING);
        }
        if (vmId.exists()) {
            vmIdBackup = File.createTempFile("instance_id", "backup");
            Files.copy(vmId.toPath(), vmIdBackup.toPath(), REPLACE_EXISTING);
        }

        createUncachedVmWithName("test_vm_rerun", normalConfig, vmIdBackup, vmInstanceBackup);
        assertThat(tryBootVm(TAG, "test_vm_rerun").payloadStarted).isTrue();

        // Launch the same VM with a different debug level. The Java API prohibits this
        // (thankfully).
        // For testing, we do that by creating a new VM with debug level, and overwriting the old
        // instance data to the new VM instance data.
        VirtualMachineConfig debugConfig = builder.setDebugLevel(toLevel).build();
        createUncachedVmWithName(
                "test_vm_changed_debug_level", debugConfig, vmIdBackup, vmInstanceBackup);
        assertThat(tryBootVm(TAG, "test_vm_changed_debug_level").payloadStarted).isFalse();
    }

    private static class VmCdis {
        public byte[] cdiAttest;
        public byte[] instanceSecret;
    }

    private VmCdis launchVmAndGetCdis(String instanceName) throws Exception {
        VirtualMachine vm = getVirtualMachineManager().get(instanceName);
        VmCdis vmCdis = new VmCdis();
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        try {
                            ITestService testService =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            vmCdis.cdiAttest = testService.insecurelyExposeAttestationCdi();
                            vmCdis.instanceSecret = testService.insecurelyExposeVmInstanceSecret();
                        } catch (Exception e) {
                            exception.complete(e);
                        } finally {
                            forceStop(vm);
                        }
                    }
                };
        listener.runToFinish(TAG, vm);
        Exception e = exception.getNow(null);
        if (e != null) {
            throw new RuntimeException(e);
        }
        return vmCdis;
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @GmsTest(requirements = {"GMS-3-7.1-011"})
    public void instancesOfSameVmHaveDifferentCdis() throws Exception {
        assumeSupportedDevice();
        // TODO(b/325094712): VMs on CF with same payload have the same secret. This is because
        // `instance-id` which is input to DICE is contained in DT which is missing in CF.
        assumeFalse(
                "Cuttlefish/Goldfish doesn't support device tree under /proc/device-tree",
                isCuttlefish() || isGoldfish());

        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig normalConfig =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config.json")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        forceCreateNewVirtualMachine("test_vm_a", normalConfig);
        forceCreateNewVirtualMachine("test_vm_b", normalConfig);
        VmCdis vm_a_cdis = launchVmAndGetCdis("test_vm_a");
        VmCdis vm_b_cdis = launchVmAndGetCdis("test_vm_b");
        assertThat(vm_a_cdis.cdiAttest).isNotNull();
        assertThat(vm_b_cdis.cdiAttest).isNotNull();
        assertThat(vm_a_cdis.cdiAttest).isNotEqualTo(vm_b_cdis.cdiAttest);
        assertThat(vm_a_cdis.instanceSecret).isNotNull();
        assertThat(vm_b_cdis.instanceSecret).isNotNull();
        assertThat(vm_a_cdis.instanceSecret).isNotEqualTo(vm_b_cdis.instanceSecret);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @GmsTest(requirements = {"GMS-3-7.1-011"})
    public void sameInstanceKeepsSameCdis() throws Exception {
        assumeSupportedDevice();
        assume().withMessage("Skip on CF. Too Slow. b/257270529").that(isCuttlefish()).isFalse();

        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig normalConfig =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config.json")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        forceCreateNewVirtualMachine("test_vm", normalConfig);

        VmCdis first_boot_cdis = launchVmAndGetCdis("test_vm");
        VmCdis second_boot_cdis = launchVmAndGetCdis("test_vm");
        // The attestation CDI isn't specified to be stable, though it might be
        assertThat(first_boot_cdis.instanceSecret).isNotNull();
        assertThat(second_boot_cdis.instanceSecret).isNotNull();
        assertThat(first_boot_cdis.instanceSecret).isEqualTo(second_boot_cdis.instanceSecret);
    }

    @Test
    @VsrTest(requirements = {"VSR-7.1-001.005"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.004"})
    public void protectedVmHasValidDiceChain() throws Exception {
        // This test validates two things regarding the pVM DICE chain:
        // 1. The DICE chain is well-formed that all the entries conform to the DICE spec.
        // 2. Each entry in the DICE chain is signed by the previous entry's subject public key.
        assumeSupportedDevice();
        assumeProtectedVM();
        assumeVsrCompliant();
        assumeTrue("Vendor API must be newer than 202404", getVendorApiLevel() > 202404);

        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config.json")
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("bcc_vm_for_vsr", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (service, results) -> {
                            results.mBcc = service.getBcc();
                        });
        testResults.assertNoException();
        byte[] bccBytes = testResults.mBcc;
        assertThat(bccBytes).isNotNull();

        String buildType = SystemProperties.get("ro.build.type");
        boolean nonUserBuild = !buildType.isEmpty() && buildType != "user";

        assertThat(HwTrustJni.validateDiceChain(bccBytes, nonUserBuild)).isTrue();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void accessToCdisIsRestricted() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        forceCreateNewVirtualMachine("test_vm", config);

        assertThrows(Exception.class, () -> launchVmAndGetCdis("test_vm"));
    }

    private static final UUID MICRODROID_PARTITION_UUID =
            UUID.fromString("cf9afe9a-0662-11ec-a329-c32663a09d75");
    private static final UUID PVM_FW_PARTITION_UUID =
            UUID.fromString("90d2174a-038a-4bc6-adf3-824848fc5825");
    private static final long BLOCK_SIZE = 512;

    // Find the starting offset which holds the data of a partition having UUID.
    // This is a kind of hack; rather than parsing QCOW2 we exploit the fact that the cluster size
    // is normally greater than 512. It implies that the partition data should exist at a block
    // which follows the header block
    private OptionalLong findPartitionDataOffset(RandomAccessFile file, UUID uuid)
            throws IOException {
        // For each 512-byte block in file, check header
        long fileSize = file.length();

        for (long idx = 0; idx + BLOCK_SIZE < fileSize; idx += BLOCK_SIZE) {
            file.seek(idx);
            long high = file.readLong();
            long low = file.readLong();
            if (uuid.equals(new UUID(high, low))) return OptionalLong.of(idx + BLOCK_SIZE);
        }
        return OptionalLong.empty();
    }

    private void flipBit(RandomAccessFile file, long offset) throws IOException {
        file.seek(offset);
        int b = file.readByte();
        file.seek(offset);
        file.writeByte(b ^ 1);
    }

    private RandomAccessFile prepareInstanceImage(String vmName) throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        assertThat(tryBootVmWithConfig(config, vmName).payloadStarted).isTrue();
        File instanceImgPath = getVmFile(vmName, "instance.img");
        return new RandomAccessFile(instanceImgPath, "rw");
    }

    private void assertThatPartitionIsMissing(UUID partitionUuid) throws Exception {
        RandomAccessFile instanceFile = prepareInstanceImage("test_vm_integrity");
        assertThat(findPartitionDataOffset(instanceFile, partitionUuid).isPresent()).isFalse();
    }

    // Flips a bit of given partition, and then see if boot fails.
    private void assertThatBootFailsAfterCompromisingPartition(UUID partitionUuid)
            throws Exception {
        RandomAccessFile instanceFile = prepareInstanceImage("test_vm_integrity");
        OptionalLong offset = findPartitionDataOffset(instanceFile, partitionUuid);
        assertThat(offset.isPresent()).isTrue();

        flipBit(instanceFile, offset.getAsLong());

        BootResult result = tryBootVm(TAG, "test_vm_integrity");
        assertThat(result.payloadStarted).isFalse();

        // This failure should shut the VM down immediately and shouldn't trigger a hangup.
        assertThat(result.deathReason).isNotEqualTo(VirtualMachineCallback.STOP_REASON_HANGUP);
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-006"})
    public void bootFailsWhenMicrodroidDataIsCompromised() throws Exception {
        // If Updatable VM is supported => No instance.img required
        assumeNoUpdatableVmSupport();
        assertThatBootFailsAfterCompromisingPartition(MICRODROID_PARTITION_UUID);
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-006"})
    public void bootFailsWhenPvmFwDataIsCompromised() throws Exception {
        // If Updatable VM is supported => No instance.img required
        assumeNoUpdatableVmSupport();
        if (mProtectedVm) {
            assertThatBootFailsAfterCompromisingPartition(PVM_FW_PARTITION_UUID);
        } else {
            // non-protected VM shouldn't have pvmfw data
            assertThatPartitionIsMissing(PVM_FW_PARTITION_UUID);
        }
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-006"})
    public void bootFailsWhenConfigIsInvalid() throws Exception {
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_no_task.json")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        BootResult bootResult = tryBootVmWithConfig(config, "test_vm_invalid_config");
        assertThat(bootResult.payloadStarted).isFalse();
        assertThat(bootResult.deathReason)
                .isEqualTo(VirtualMachineCallback.STOP_REASON_MICRODROID_INVALID_PAYLOAD_CONFIG);
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-006"})
    public void bootFailsWhenBinaryNameIsInvalid() throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("DoesNotExist.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        BootResult bootResult = tryBootVmWithConfig(config, "test_vm_invalid_binary_path");
        assertThat(bootResult.payloadStarted).isFalse();
        assertThat(bootResult.deathReason)
                .isEqualTo(VirtualMachineCallback.STOP_REASON_MICRODROID_UNKNOWN_RUNTIME_ERROR);
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-006"})
    public void bootFailsWhenApkPathIsInvalid() {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setApkPath("/does/not/exist")
                        .build();

        assertThrowsVmException(
                () -> tryBootVmWithConfig(config, "test_vm_invalid_apk_path"),
                VirtualMachineException.CODE_PAYLOAD_CONFIG_MALFORMED,
                "Failed to open APK");
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-006"})
    public void bootFailsWhenExtraApkPackageIsInvalid() {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .addExtraApk("com.example.nosuch.package")
                        .build();
        assertThrowsVmException(
                () -> tryBootVmWithConfig(config, "test_vm_invalid_extra_apk_package"),
                VirtualMachineException.CODE_PAYLOAD_CONFIG_MALFORMED,
                "Extra APK package not found");
    }

    private BootResult tryBootVmWithConfig(VirtualMachineConfig config, String vmName)
            throws Exception {
        try (VirtualMachine ignored = forceCreateNewVirtualMachine(vmName, config)) {
            return tryBootVm(TAG, vmName);
        }
    }

    // Checks whether microdroid_launcher started but payload failed. reason must be recorded in the
    // console output.
    private void assertThatPayloadFailsDueTo(VirtualMachine vm, String reason) throws Exception {
        final CompletableFuture<Boolean> payloadStarted = new CompletableFuture<>();
        final CompletableFuture<Integer> exitCodeFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadStarted(VirtualMachine vm) {
                        payloadStarted.complete(true);
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        exitCodeFuture.complete(exitCode);
                    }
                };
        listener.runToFinish(TAG, vm);

        assertThat(payloadStarted.getNow(false)).isTrue();
        assertThat(exitCodeFuture.getNow(0)).isNotEqualTo(0);
        assertThat(listener.getConsoleOutput() + listener.getLogOutput()).contains(reason);
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-006"})
    public void bootFailsWhenBinaryIsMissingEntryFunction() throws Exception {
        VirtualMachineConfig normalConfig =
                newVmConfigBuilderWithPayloadBinary("MicrodroidEmptyNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setVmOutputCaptured(true)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_missing_entry", normalConfig);

        assertThatPayloadFailsDueTo(vm, "Failed to find entrypoint");
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-006"})
    public void bootFailsWhenBinaryTriesToLinkAgainstPrivateLibs() throws Exception {
        VirtualMachineConfig normalConfig =
                newVmConfigBuilderWithPayloadBinary("MicrodroidPrivateLinkingNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setVmOutputCaptured(true)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_private_linking", normalConfig);

        assertThatPayloadFailsDueTo(vm, "Failed to dlopen");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void sameInstancesShareTheSameVmObject() throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so").build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        VirtualMachine vm2 = getVirtualMachineManager().get("test_vm");
        assertThat(vm).isEqualTo(vm2);

        VirtualMachine newVm = forceCreateNewVirtualMachine("test_vm", config);
        VirtualMachine newVm2 = getVirtualMachineManager().get("test_vm");
        assertThat(newVm).isEqualTo(newVm2);

        assertThat(vm).isNotEqualTo(newVm);
    }

    @Test
    @CddTest
    public void vmNotUpdatableWithLegacyRpMechanism() throws Exception {
        // Legacy secret management (which involves storing code hashes in instance.img) is expected
        // to decline VM run with different code, which includes updated code hash.
        // Failure to comply may indicate broken rollback protection.
        assumeSupportedDevice();
        assumeProtectedVM();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V5.apk");

        Context testHelperAppCtx =
                getContext()
                        .createPackageContext(
                                RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME, 0);
        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(testHelperAppCtx)
                        .setDisableUpdatability()
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setPayloadBinaryName("MicrodroidTestNativeLib.so")
                        .setProtectedVm(isProtectedVm())
                        .setOs(os())
                        .setEncryptedStorageBytes(1 * 1024 * 1024)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("legacy_rp_update", config);

        Path vmInstance = getVmFile("legacy_rp_update", "instance.img").toPath();
        Path vmInstanceBackup = File.createTempFile("instance", ".img").toPath();
        Files.copy(vmInstance, vmInstanceBackup, REPLACE_EXISTING);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            ts.writeToFile(
                                    /* content= */ EXAMPLE_STRING,
                                    /* path= */ "/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();

        // Reset the instance img that tracks the code hashes across VM boot - an attempt to test
        // the code hash checks is intact.
        Files.copy(vmInstanceBackup, vmInstance, REPLACE_EXISTING);

        // Install an updated payload!
        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V7_inc_rollback_version.apk");
        // Now pVM shouldn't boot.
        BootResult bootResult = tryBootVm(TAG, vm);
        assertThat(bootResult.payloadStarted).isFalse();
    }

    @Test
    @CddTest
    public void importedVmAndOriginalVmHaveTheSameCdi() throws Exception {
        assumeSupportedDevice();
        // Arrange
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config.json")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        String vmNameOrig = "test_vm_orig";
        String vmNameImport = "test_vm_import";
        VirtualMachine vmOrig = forceCreateNewVirtualMachine(vmNameOrig, config);
        VmCdis origCdis = launchVmAndGetCdis(vmNameOrig);
        assertThat(origCdis.instanceSecret).isNotNull();
        VirtualMachineManager vmm = getVirtualMachineManager();
        if (vmm.get(vmNameImport) != null) {
            vmm.delete(vmNameImport);
        }

        // Action
        // The imported VM will be fetched by name later.
        vmm.importFromDescriptor(vmNameImport, vmOrig.toDescriptor());

        // Asserts
        VmCdis importCdis = launchVmAndGetCdis(vmNameImport);
        assertThat(origCdis.instanceSecret).isEqualTo(importCdis.instanceSecret);
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1"})
    public void importedVmIsEqualToTheOriginalVm_WithoutStorage() throws Exception {
        TestResults testResults = importedVmIsEqualToTheOriginalVm(false);
        assertThat(testResults.mEncryptedStoragePath).isEqualTo("");
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1"})
    public void importedVmIsEqualToTheOriginalVm_WithStorage() throws Exception {
        TestResults testResults = importedVmIsEqualToTheOriginalVm(true);
        assertThat(testResults.mEncryptedStoragePath).isEqualTo("/mnt/encryptedstore");
    }

    private TestResults importedVmIsEqualToTheOriginalVm(boolean encryptedStoreEnabled)
            throws Exception {
        // Arrange
        VirtualMachineConfig.Builder builder =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL);
        if (encryptedStoreEnabled) {
            builder.setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES);
        }
        VirtualMachineConfig config = builder.build();
        String vmNameOrig = "test_vm_orig";
        String vmNameImport = "test_vm_import";
        VirtualMachine vmOrig = forceCreateNewVirtualMachine(vmNameOrig, config);
        // Run something to make the instance.img different with the initialized one.
        TestResults origTestResults =
                runVmTestService(
                        TAG,
                        vmOrig,
                        (ts, tr) -> {
                            tr.mAddInteger = ts.addInteger(123, 456);
                            tr.mEncryptedStoragePath = ts.getEncryptedStoragePath();
                        });
        origTestResults.assertNoException();
        assertThat(origTestResults.mAddInteger).isEqualTo(123 + 456);
        VirtualMachineManager vmm = getVirtualMachineManager();
        if (vmm.get(vmNameImport) != null) {
            vmm.delete(vmNameImport);
        }

        // Action
        VirtualMachine vmImport = vmm.importFromDescriptor(vmNameImport, vmOrig.toDescriptor());

        // Asserts
        assertFileContentsAreEqualInTwoVms("config.xml", vmNameOrig, vmNameImport);
        assertFileContentsAreEqualInTwoVms("instance.img", vmNameOrig, vmNameImport);
        if (encryptedStoreEnabled) {
            assertFileContentsAreEqualInTwoVms("storage.img", vmNameOrig, vmNameImport);
        }
        assertThat(vmImport).isNotEqualTo(vmOrig);
        assertThat(vmImport).isEqualTo(vmm.get(vmNameImport));
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vmImport,
                        (ts, tr) -> {
                            tr.mAddInteger = ts.addInteger(123, 456);
                            tr.mEncryptedStoragePath = ts.getEncryptedStoragePath();
                        });
        testResults.assertNoException();
        assertThat(testResults.mAddInteger).isEqualTo(123 + 456);
        return testResults;
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void encryptedStorageAvailable() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mEncryptedStoragePath = ts.getEncryptedStoragePath();
                        });
        assertThat(testResults.mEncryptedStoragePath).isEqualTo("/mnt/encryptedstore");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void encryptedStorageIsInaccessibleToDifferentVm() throws Exception {
        assumeSupportedDevice();
        // TODO(b/325094712): VMs on CF with same payload have the same secret. This is because
        // `instance-id` which is input to DICE is contained in DT which is missing in CF.
        assumeFalse(
                "Cuttlefish/Goldfish doesn't support device tree under /proc/device-tree",
                isCuttlefish() || isGoldfish());

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            ts.writeToFile(
                                    /* content= */ EXAMPLE_STRING,
                                    /* path= */ "/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();

        // Start a different vm (this changes the vm identity)
        VirtualMachine diff_test_vm = forceCreateNewVirtualMachine("diff_test_vm", config);

        // Replace the backing storage image to the original one
        File storageImgOrig = getVmFile("test_vm", "storage.img");
        File storageImgNew = getVmFile("diff_test_vm", "storage.img");
        Files.copy(storageImgOrig.toPath(), storageImgNew.toPath(), REPLACE_EXISTING);
        assertFileContentsAreEqualInTwoVms("storage.img", "test_vm", "diff_test_vm");

        CompletableFuture<Boolean> onPayloadReadyExecuted = new CompletableFuture<>();
        CompletableFuture<Boolean> onErrorExecuted = new CompletableFuture<>();
        CompletableFuture<String> errorMessage = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        onPayloadReadyExecuted.complete(true);
                        super.onPayloadReady(vm);
                    }

                    @Override
                    public void onError(VirtualMachine vm, int errorCode, String message) {
                        onErrorExecuted.complete(true);
                        errorMessage.complete(message);
                        super.onError(vm, errorCode, message);
                    }
                };
        listener.runToFinish(TAG, diff_test_vm);

        // Assert that payload never started & error message reflects storage error.
        assertThat(onPayloadReadyExecuted.getNow(false)).isFalse();
        assertThat(onErrorExecuted.getNow(false)).isTrue();
        assertThat(errorMessage.getNow("")).contains("Unable to prepare encrypted storage");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void microdroidLauncherHasEmptyCapabilities() throws Exception {
        assumeSupportedDevice();

        final VirtualMachineConfig vmConfig =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        final VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_caps", vmConfig);

        final TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mEffectiveCapabilities = ts.getEffectiveCapabilities();
                        });

        testResults.assertNoException();
        assertThat(testResults.mEffectiveCapabilities).isEmpty();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @GmsTest(requirements = {"GMS-3-7.1-005"})
    public void payloadIsNotRoot() throws Exception {
        assumeSupportedDevice();
        assumeTrue("MultiTenancy feature not supported", isFeatureMultiTenantSupported());
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mUid = ts.getUid();
                        });
        testResults.assertNoException();
        assertThat(testResults.mUid).isNotEqualTo(0);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void encryptedStorageIsPersistent() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_a", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            ts.writeToFile(
                                    /* content= */ EXAMPLE_STRING,
                                    /* path= */ "/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();

        // Re-run the same VM & verify the file persisted. Note, the previous `runVmTestService`
        // stopped the VM
        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mFileContent = ts.readFromFile("/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();
        assertThat(testResults.mFileContent).isEqualTo(EXAMPLE_STRING);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void encryptedStorageSupportsExpansion() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mEncryptedStorageSize = ts.getEncryptedStorageSize();
                        });
        testResults.assertNoException();
        assertThat(testResults.mEncryptedStorageSize)
            .isWithin(TOLERANCE_BYTES)
            .of(ENCRYPTED_STORAGE_BYTES);

        // Re-run the VM with more storage size & verify the file persisted.
        // Note, the previous `runVmTestService` stopped the VM
        config = newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                    .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES * 2)
                    .build();
        vm.setConfig(config);
        assertThat(vm.getConfig().getEncryptedStorageBytes())
            .isEqualTo(ENCRYPTED_STORAGE_BYTES * 2);

        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mEncryptedStorageSize = ts.getEncryptedStorageSize();
                        });
        testResults.assertNoException();
        assertThat(testResults.mEncryptedStorageSize)
            .isWithin(TOLERANCE_BYTES)
            .of(ENCRYPTED_STORAGE_BYTES * 2);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void encryptedStorageExpansionIsPersistent() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            ts.writeToFile(
                                    /* content= */ EXAMPLE_STRING,
                                    /* path= */ "/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();

        // Re-run the VM with more storage size & verify the file persisted.
        // Note, the previous `runVmTestService` stopped the VM
        config = newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                    .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES * 2)
                    .build();
        vm.setConfig(config);

        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mFileContent = ts.readFromFile("/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();
        assertThat(testResults.mFileContent).isEqualTo(EXAMPLE_STRING);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void encryptedStorageSizeUnchanged() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mEncryptedStorageSize = ts.getEncryptedStorageSize();
                        });
        testResults.assertNoException();
        assertThat(testResults.mEncryptedStorageSize)
            .isWithin(TOLERANCE_BYTES)
            .of(ENCRYPTED_STORAGE_BYTES);

        // Re-run the VM with more storage size & verify the file persisted.
        // Note, the previous `runVmTestService` stopped the VM
        config = newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                    .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                    .build();
        vm.setConfig(config);
        assertThat(vm.getConfig().getEncryptedStorageBytes())
            .isEqualTo(ENCRYPTED_STORAGE_BYTES);

        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mEncryptedStorageSize = ts.getEncryptedStorageSize();
                        });
        testResults.assertNoException();
        assertThat(testResults.mEncryptedStorageSize)
            .isWithin(TOLERANCE_BYTES)
            .of(ENCRYPTED_STORAGE_BYTES);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void encryptedStorageShrinkFails() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mEncryptedStorageSize = ts.getEncryptedStorageSize();
                        });
        testResults.assertNoException();
        assertThat(testResults.mEncryptedStorageSize)
            .isWithin(TOLERANCE_BYTES)
            .of(ENCRYPTED_STORAGE_BYTES);

        // Re-run the VM with more storage size & verify the file persisted.
        // Note, the previous `runVmTestService` stopped the VM
        VirtualMachineConfig newConfig =
            newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                    .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES / 2)
                    .build();
        assertThrowsVmException(
                () -> vm.setConfig(newConfig),
                VirtualMachineException.CODE_CONFIG_INCOMPATIBLE,
                "incompatible config");
    }

    private boolean deviceCapableOfProtectedVm() {
        int capabilities = getVirtualMachineManager().getCapabilities();
        if ((capabilities & CAPABILITY_PROTECTED_VM) != 0) {
            return true;
        }
        return false;
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void rollbackProtectedDataOfPayload() throws Exception {
        assumeSupportedDevice();
        // Rollback protected data is only possible if Updatable VMs is supported -
        // which implies Secretkeeper support.
        assumeTrue("Missing Updatable VM support", isUpdatableVmSupported());

        byte[] value1 = new byte[32];
        Arrays.fill(value1, (byte) 0xcc);
        byte[] value2 = new byte[32];
        Arrays.fill(value2, (byte) 0xdd);

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mPayloadRpData = ts.insecurelyReadPayloadRpData();
                        });
        // `insecurelyReadPayloadRpData()` must've failed since no data was ever written!
        assertWithMessage("The read (unexpectedly) succeeded!")
                .that(testResults.mException)
                .isNotNull();

        // Re-run the same VM & write/read th RP data & verify it what we just wrote!
        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            ts.insecurelyWritePayloadRpData(value1);
                            tr.mPayloadRpData = ts.insecurelyReadPayloadRpData();
                            ts.insecurelyWritePayloadRpData(value2);
                        });
        testResults.assertNoException();
        assertThat(testResults.mPayloadRpData).isEqualTo(value1);

        // Re-run the same VM again
        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mPayloadRpData = ts.insecurelyReadPayloadRpData();
                        });
        testResults.assertNoException();
        assertThat(testResults.mPayloadRpData).isEqualTo(value2);
    }

    @Test
    public void rollbackProtectedDataCanBeAccessedPostConnectionExpiration() throws Exception {
        assumeSupportedDevice();
        // Rollback protected data is only possible if Updatable VMs is supported -
        // which implies Secretkeeper support.
        assumeTrue("Missing Updatable VM support", isUpdatableVmSupported());

        final long vmSize = minMemoryRequired();
        // The reference implementation of Secretkeeper maintains 4 live session keys,
        // dropping the oldest one when new connections are requested. Therefore we spin 8 VMs
        // asynchronously.
        // Within a VM, wait for 5 sec (> Microdroid boot time) and trigger rp data access
        // hoping at least some of the connection between VM <-> Secretkeeper are expired.
        final int numVMs = 8;
        final long availableMem = getAvailableMemory();

        // Let's not use more than half of the available memory
        assume().withMessage("Available memory (" + availableMem + " bytes) too small")
                .that((numVMs * vmSize) <= (availableMem / 2))
                .isTrue();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setMemoryBytes(vmSize)
                        .build();
        byte[] data = new byte[32];
        Arrays.fill(data, (byte) 0xcc);

        CompletableFuture<TestResults>[] resultFutureList = new CompletableFuture[numVMs];
        for (int i = 0; i < numVMs; i++) {
            final VirtualMachine vm =
                    forceCreateNewVirtualMachine("test_sk_session_expiration_vm_" + i, config);
            resultFutureList[i] =
                    CompletableFuture.supplyAsync(
                            () -> {
                                try {
                                    TestResults testResults =
                                            runVmTestService(
                                                    TAG,
                                                    vm,
                                                    (ts, tr) -> {
                                                        ts.insecurelyWritePayloadRpData(data);
                                                        Thread.sleep(5 * 1000); // 5 seconds of wait
                                                        tr.mPayloadRpData =
                                                                ts.insecurelyReadPayloadRpData();
                                                    });
                                    return testResults;
                                } catch (Exception e) {
                                    throw new CompletionException(e);
                                }
                            });
        }

        for (int i = 0; i < numVMs; i++) {
            TestResults testResult = resultFutureList[i].get();
            testResult.assertNoException();
            assertThat(testResult.mPayloadRpData).isEqualTo(data);
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void isNewInstanceTest() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        // TODO(b/325094712): Cuttlefish doesn't support device tree overlays which is required to
        // find if the VM run is a new instance.
        assumeFalse(
                "Cuttlefish/Goldfish doesn't support device tree under /proc/device-tree",
                isCuttlefish() || isGoldfish());
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_a", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mIsNewInstance = ts.isNewInstance();
                        });
        testResults.assertNoException();
        assertThat(testResults.mIsNewInstance).isTrue();

        // Re-run the same VM & ensure isNewInstance is false.
        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mIsNewInstance = ts.isNewInstance();
                        });
        testResults.assertNoException();
        assertThat(testResults.mIsNewInstance).isFalse();
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
    public void canReadFileFromAssets_debugFull() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_read_from_assets", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (testService, ts) -> {
                            ts.mFileContent = testService.readFromFile("/mnt/apk/assets/file.txt");
                        });

        testResults.assertNoException();
        assertThat(testResults.mFileContent).isEqualTo("Hello, I am a file!");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void outputShouldBeExplicitlyCaptured() throws Exception {
        assumeSupportedDevice();

        final VirtualMachineConfig vmConfig =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setVmConsoleInputSupported(true) // even if console input is supported
                        .build();
        final VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_forward_log", vmConfig);
        vm.run();

        try {
            assertThrowsVmException(
                    () -> vm.getConsoleOutput(),
                    VirtualMachineException.CODE_FEATURE_DISABLED,
                    "Capturing vm outputs is turned off");
            assertThrowsVmException(
                    () -> vm.getLogOutput(),
                    VirtualMachineException.CODE_FEATURE_DISABLED,
                    "Capturing vm outputs is turned off");
        } finally {
            vm.stop();
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void inputShouldBeExplicitlyAllowed() throws Exception {
        assumeSupportedDevice();

        final VirtualMachineConfig vmConfig =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setVmOutputCaptured(true) // even if output is captured
                        .build();
        final VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_forward_log", vmConfig);
        vm.run();

        try {
            assertThrowsVmException(
                    () -> vm.getConsoleInput(),
                    VirtualMachineException.CODE_FEATURE_DISABLED,
                    "VM console input is not supported");
        } finally {
            vm.stop();
        }
    }

    private boolean checkVmOutputIsRedirectedToLogcat(boolean debuggable) throws Exception {
        String time =
                LocalDateTime.now().format(DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss.SSS"));
        final VirtualMachineConfig vmConfig =
                new VirtualMachineConfig.Builder(getContext())
                        .setProtectedVm(mProtectedVm)
                        .setPayloadBinaryName("MicrodroidTestNativeLib.so")
                        .setDebugLevel(debuggable ? DEBUG_LEVEL_FULL : DEBUG_LEVEL_NONE)
                        .setVmOutputCaptured(false)
                        .setOs(os())
                        .build();
        final VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_logcat", vmConfig);

        runVmTestService(TAG, vm, (service, results) -> {});

        // only check logs printed after this test
        Process logcatProcess =
                new ProcessBuilder()
                        .command(
                                "logcat",
                                "-e",
                                "virtualizationmanager.* (Console|Log).*executing main task",
                                "-t",
                                time)
                        .start();
        logcatProcess.waitFor();
        BufferedReader reader =
                new BufferedReader(new InputStreamReader(logcatProcess.getInputStream()));
        return !Strings.isNullOrEmpty(reader.readLine());
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void outputIsRedirectedToLogcatIfNotCaptured() throws Exception {
        assumeSupportedDevice();

        assertThat(checkVmOutputIsRedirectedToLogcat(true)).isTrue();
    }

    private boolean isDebugPolicyPossiblyEnabled(String entry) {
        Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
        UiAutomation uiAutomation = instrumentation.getUiAutomation();
        String cmd = "/apex/com.android.virt/bin/vm info";
        String output = runInShellWithStderr(TAG, uiAutomation, cmd).trim();
        for (String line : output.split("\\v")) {
            if (line.matches("^.*Debug policy.*$")) {
                return line.matches("^.*" + entry + ": true.*$");
            }
        }

        // If the test is running on the older device before `vm info` dumps debug policy,
        // then there's no solid way to know whether debug policy is enabled or not in user build.
        // Just return true to skip the test.
        return true;
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void outputIsNotRedirectedToLogcatIfNotDebuggable() throws Exception {
        assumeSupportedDevice();

        // Ensure that debug policy isn't enabled to *always log*.
        assumeFalse(isDebugPolicyPossiblyEnabled("log"));

        assertThat(checkVmOutputIsRedirectedToLogcat(false)).isFalse();
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void testConsoleInputSupported() throws Exception {
        assumeSupportedDevice();
        assumeFalse("Not supported on GKI kernels", mOs.startsWith("microdroid_gki-"));

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setVmConsoleInputSupported(true)
                        .setVmOutputCaptured(true)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_console_in", config);

        final String TYPED = "this is a console input\n";
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            OutputStreamWriter consoleIn =
                                    new OutputStreamWriter(vm.getConsoleInput());
                            consoleIn.write(TYPED);
                            consoleIn.close();
                            tr.mConsoleInput = ts.readLineFromConsole();
                        });
        testResults.assertNoException();
        assertThat(testResults.mConsoleInput).isEqualTo(TYPED);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void testStartVmWithPayloadOfAnotherApp() throws Exception {
        assumeSupportedDevice();

        Context ctx = getContext();
        Context otherAppCtx = ctx.createPackageContext(VM_SHARE_APP_PACKAGE_NAME, 0);

        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(otherAppCtx)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setProtectedVm(isProtectedVm())
                        .setPayloadBinaryName("MicrodroidPayloadInOtherAppNativeLib.so")
                        .setOs(os())
                        .build();

        try (VirtualMachine vm = forceCreateNewVirtualMachine("vm_from_another_app", config)) {
            TestResults results =
                    runVmTestService(
                            TAG,
                            vm,
                            (ts, tr) -> {
                                tr.mAddInteger = ts.addInteger(101, 303);
                            });
            assertThat(results.mAddInteger).isEqualTo(404);
        }

        getVirtualMachineManager().delete("vm_from_another_app");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void testVmDescriptorParcelUnparcel_noTrustedStorage() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        VirtualMachine originalVm = forceCreateNewVirtualMachine("original_vm", config);
        // Just start & stop the VM.
        runVmTestService(TAG, originalVm, (ts, tr) -> {});

        // Now create the descriptor and manually parcel & unparcel it.
        VirtualMachineDescriptor vmDescriptor = toParcelFromParcel(originalVm.toDescriptor());

        if (getVirtualMachineManager().get("import_vm_from_unparceled") != null) {
            getVirtualMachineManager().delete("import_vm_from_unparceled");
        }

        VirtualMachine importVm =
                getVirtualMachineManager()
                        .importFromDescriptor("import_vm_from_unparceled", vmDescriptor);

        assertFileContentsAreEqualInTwoVms(
                "config.xml", "original_vm", "import_vm_from_unparceled");
        assertFileContentsAreEqualInTwoVms(
                "instance.img", "original_vm", "import_vm_from_unparceled");

        // Check that we can start and stop imported vm as well
        runVmTestService(TAG, importVm, (ts, tr) -> {});
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void testVmDescriptorParcelUnparcel_withTrustedStorage() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setEncryptedStorageBytes(1_000_000)
                        .build();

        VirtualMachine originalVm = forceCreateNewVirtualMachine("original_vm", config);
        // Just start & stop the VM.
        {
            TestResults testResults =
                    runVmTestService(
                            TAG,
                            originalVm,
                            (ts, tr) -> {
                                ts.writeToFile("not a secret!", "/mnt/encryptedstore/secret.txt");
                            });
            assertThat(testResults.mException).isNull();
        }

        // Now create the descriptor and manually parcel & unparcel it.
        VirtualMachineDescriptor vmDescriptor = toParcelFromParcel(originalVm.toDescriptor());

        if (getVirtualMachineManager().get("import_vm_from_unparceled") != null) {
            getVirtualMachineManager().delete("import_vm_from_unparceled");
        }

        VirtualMachine importVm =
                getVirtualMachineManager()
                        .importFromDescriptor("import_vm_from_unparceled", vmDescriptor);

        assertFileContentsAreEqualInTwoVms(
                "config.xml", "original_vm", "import_vm_from_unparceled");
        assertFileContentsAreEqualInTwoVms(
                "instance.img", "original_vm", "import_vm_from_unparceled");
        assertFileContentsAreEqualInTwoVms(
                "storage.img", "original_vm", "import_vm_from_unparceled");

        TestResults testResults =
                runVmTestService(
                        TAG,
                        importVm,
                        (ts, tr) -> {
                            tr.mFileContent = ts.readFromFile("/mnt/encryptedstore/secret.txt");
                        });

        assertThat(testResults.mException).isNull();
        assertThat(testResults.mFileContent).isEqualTo("not a secret!");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void testShareVmWithAnotherApp() throws Exception {
        assumeSupportedDevice();

        Context ctx = getContext();
        Context otherAppCtx = ctx.createPackageContext(VM_SHARE_APP_PACKAGE_NAME, 0);

        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(otherAppCtx)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setProtectedVm(isProtectedVm())
                        .setPayloadBinaryName("MicrodroidPayloadInOtherAppNativeLib.so")
                        .setOs(os())
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("vm_to_share", config);
        // Just start & stop the VM.
        runVmTestService(TAG, vm, (ts, tr) -> {});
        // Get a descriptor that we will share with another app (VM_SHARE_APP_PACKAGE_NAME)
        VirtualMachineDescriptor vmDesc = vm.toDescriptor();

        Intent serviceIntent = new Intent();
        serviceIntent.setComponent(
                new ComponentName(
                        VM_SHARE_APP_PACKAGE_NAME,
                        "com.android.microdroid.test.sharevm.VmShareServiceImpl"));
        serviceIntent.setAction("com.android.microdroid.test.sharevm.VmShareService");

        VmShareServiceConnection connection = new VmShareServiceConnection();
        boolean ret = ctx.bindService(serviceIntent, connection, Context.BIND_AUTO_CREATE);
        assertWithMessage("Failed to bind to " + serviceIntent).that(ret).isTrue();

        IVmShareTestService service = connection.waitForService();
        assertWithMessage("Timed out connecting to " + serviceIntent).that(service).isNotNull();

        try {
            ITestService testServiceProxy = transferAndStartVm(service, vmDesc, "vm_to_share");

            int result = testServiceProxy.addInteger(37, 73);
            assertThat(result).isEqualTo(110);
        } finally {
            ctx.unbindService(connection);
        }
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void testShareVmWithAnotherApp_encryptedStorage() throws Exception {
        assumeSupportedDevice();

        Context ctx = getContext();
        Context otherAppCtx = ctx.createPackageContext(VM_SHARE_APP_PACKAGE_NAME, 0);

        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(otherAppCtx)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setProtectedVm(isProtectedVm())
                        .setEncryptedStorageBytes(3_000_000)
                        .setPayloadBinaryName("MicrodroidPayloadInOtherAppNativeLib.so")
                        .setOs(os())
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("vm_to_share", config);
        // Just start & stop the VM.
        runVmTestService(
                TAG,
                vm,
                (ts, tr) -> {
                    ts.writeToFile(EXAMPLE_STRING, "/mnt/encryptedstore/private.key");
                });
        // Get a descriptor that we will share with another app (VM_SHARE_APP_PACKAGE_NAME)
        VirtualMachineDescriptor vmDesc = vm.toDescriptor();

        Intent serviceIntent = new Intent();
        serviceIntent.setComponent(
                new ComponentName(
                        VM_SHARE_APP_PACKAGE_NAME,
                        "com.android.microdroid.test.sharevm.VmShareServiceImpl"));
        serviceIntent.setAction("com.android.microdroid.test.sharevm.VmShareService");

        VmShareServiceConnection connection = new VmShareServiceConnection();
        boolean ret = ctx.bindService(serviceIntent, connection, Context.BIND_AUTO_CREATE);
        assertWithMessage("Failed to bind to " + serviceIntent).that(ret).isTrue();

        IVmShareTestService service = connection.waitForService();
        assertWithMessage("Timed out connecting to " + serviceIntent).that(service).isNotNull();

        try {
            ITestService testServiceProxy = transferAndStartVm(service, vmDesc, "vm_to_share");
            String result = testServiceProxy.readFromFile("/mnt/encryptedstore/private.key");
            assertThat(result).isEqualTo(EXAMPLE_STRING);
        } finally {
            ctx.unbindService(connection);
        }
    }

    private ITestService transferAndStartVm(
            IVmShareTestService service, VirtualMachineDescriptor vmDesc, String vmName)
            throws Exception {
        // Send the VM descriptor to the other app. When received, it will reconstruct the VM
        // from the descriptor.
        service.importVm(vmDesc);

        // Now that the VM has been imported, we should be free to delete our copy (this is
        // what we recommend for VM transfer).
        getVirtualMachineManager().delete(vmName);

        // Ask the other app to start the imported VM, connect to the ITestService in it, create
        // a "proxy" ITestService binder that delegates all the calls to the VM, and share it
        // with this app. It will allow us to verify assertions on the running VM in the other
        // app.
        ITestService testServiceProxy = service.startVm();
        return testServiceProxy;
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @GmsTest(requirements = {"GMS-3-7.1-005"})
    public void testFileUnderBinHasExecutePermission() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig vmConfig =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_perms", vmConfig);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mFileMode = ts.getFilePermissions("/mnt/apk/bin/measure_io");
                        });

        testResults.assertNoException();
        int allPermissionsMask =
                OsConstants.S_IRUSR
                        | OsConstants.S_IWUSR
                        | OsConstants.S_IXUSR
                        | OsConstants.S_IRGRP
                        | OsConstants.S_IWGRP
                        | OsConstants.S_IXGRP
                        | OsConstants.S_IROTH
                        | OsConstants.S_IWOTH
                        | OsConstants.S_IXOTH;
        int expectedPermissions = OsConstants.S_IRUSR | OsConstants.S_IXUSR;
        if (isFeatureMultiTenantSupported()) {
            expectedPermissions = expectedPermissions | OsConstants.S_IRGRP | OsConstants.S_IXGRP;
        }
        assertThat(testResults.mFileMode & allPermissionsMask).isEqualTo(expectedPermissions);
    }

    // Taken from bionic/libc/kernel/uapi/linux/mount.h
    private static final int MS_RDONLY = 1;
    private static final int MS_NOEXEC = 8;
    private static final int MS_NOATIME = 1024;

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-004", "GMS-3-7.1-005"})
    public void dataIsMountedWithNoExec() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig vmConfig =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_data_mount", vmConfig);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mMountFlags = ts.getMountFlags("/data");
                        });

        assertThat(testResults.mException).isNull();
        assertWithMessage("/data should be mounted with MS_NOEXEC")
                .that(testResults.mMountFlags & MS_NOEXEC)
                .isEqualTo(MS_NOEXEC);
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-004", "GMS-3-7.1-005"})
    public void encryptedStoreIsMountedWithNoExec() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig vmConfig =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_encstore_no_exec", vmConfig);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mMountFlags = ts.getMountFlags("/mnt/encryptedstore");
                        });

        assertThat(testResults.mException).isNull();
        assertWithMessage("/mnt/encryptedstore should be mounted with MS_NOEXEC")
                .that(testResults.mMountFlags & MS_NOEXEC)
                .isEqualTo(MS_NOEXEC);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    public void createAndRunRustVm() throws Exception {
        // This test is here mostly to exercise the Rust wrapper around the VM Payload API.
        // We're testing the same functionality as in other tests, the only difference is
        // that the payload is written in Rust.

        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("libmicrodroid_testlib_rust.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("rust_vm", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mAddInteger = ts.addInteger(37, 73);
                            tr.mApkContentsPath = ts.getApkContentsPath();
                            tr.mEncryptedStoragePath = ts.getEncryptedStoragePath();
                            tr.mInstanceSecret = ts.insecurelyExposeVmInstanceSecret();
                        });
        testResults.assertNoException();
        assertThat(testResults.mAddInteger).isEqualTo(37 + 73);
        assertThat(testResults.mApkContentsPath).isEqualTo("/mnt/apk");
        assertThat(testResults.mEncryptedStoragePath).isEqualTo("");
        assertThat(testResults.mInstanceSecret).hasLength(32);
    }

    @Test
    public void createAndRunRustVmWithEncryptedStorage() throws Exception {
        // This test is here mostly to exercise the Rust wrapper around the VM Payload API.
        // We're testing the same functionality as in other tests, the only difference is
        // that the payload is written in Rust.

        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("libmicrodroid_testlib_rust.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("rust_vm", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> tr.mEncryptedStoragePath = ts.getEncryptedStoragePath());
        testResults.assertNoException();
        assertThat(testResults.mEncryptedStoragePath).isEqualTo("/mnt/encryptedstore");
    }

    private VirtualMachineConfig buildVmConfigWithVendor(File vendorDiskImage) throws Exception {
        return buildVmConfigWithVendor(vendorDiskImage, "MicrodroidTestNativeLib.so");
    }

    private VirtualMachineConfig buildVmConfigWithVendor(File vendorDiskImage, String binaryPath)
            throws Exception {
        assumeSupportedDevice();
        // TODO(b/325094712): Boot fails with vendor partition in Cuttlefish.
        assumeFalse(
                "Cuttlefish/Goldfish doesn't support device tree under /proc/device-tree",
                isCuttlefish() || isGoldfish());
        // TODO(b/317567210): Boot fails with vendor partition in HWASAN enabled microdroid
        // after introducing verification based on DT and fstab in microdroid vendor partition.
        assumeFalse(
                "boot with vendor partition is failing in HWASAN enabled Microdroid.", isHwasan());
        assumeFeatureEnabled(VirtualMachineManager.FEATURE_VENDOR_MODULES);
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary(binaryPath)
                        .setVendorDiskImage(vendorDiskImage)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        return config;
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.007"})
    @VsrTest(requirements = {"VSR-7.1-001.008"})
    public void configuringVendorDiskImageRequiresCustomPermission() throws Exception {
        File vendorDiskImage =
                new File("/data/local/tmp/cts/microdroid/test_microdroid_vendor_image.img");
        VirtualMachineConfig config = buildVmConfigWithVendor(vendorDiskImage);
        revokePermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);

        VirtualMachine vm =
                forceCreateNewVirtualMachine("test_vendor_image_req_custom_permission", config);
        SecurityException e =
                assertThrows(
                        SecurityException.class, () -> runVmTestService(TAG, vm, (ts, tr) -> {}));
        assertThat(e)
                .hasMessageThat()
                .contains("android.permission.USE_CUSTOM_VIRTUAL_MACHINE permission");
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.007"})
    @VsrTest(requirements = {"VSR-7.1-001.008"})
    public void bootsWithVendorPartition() throws Exception {
        File vendorDiskImage = new File("/vendor/etc/avf/microdroid/microdroid_vendor.img");
        assumeTrue("Microdroid vendor image doesn't exist, skip", vendorDiskImage.exists());
        VirtualMachineConfig config = buildVmConfigWithVendor(vendorDiskImage);

        VirtualMachine vm = forceCreateNewVirtualMachine("test_boot_with_vendor", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mMountFlags = ts.getMountFlags("/vendor");
                        });
        assertThat(testResults.mException).isNull();
        int expectedFlags = MS_NOATIME | MS_RDONLY;
        assertThat(testResults.mMountFlags & expectedFlags).isEqualTo(expectedFlags);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.007"})
    @VsrTest(requirements = {"VSR-7.1-001.008"})
    public void bootsWithCustomVendorPartitionForNonPvm() throws Exception {
        assumeNonProtectedVM();
        File vendorDiskImage =
                new File("/data/local/tmp/cts/microdroid/test_microdroid_vendor_image.img");
        VirtualMachineConfig config = buildVmConfigWithVendor(vendorDiskImage);

        VirtualMachine vm =
                forceCreateNewVirtualMachine("test_boot_with_custom_vendor_non_pvm", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mMountFlags = ts.getMountFlags("/vendor");
                        });
        assertThat(testResults.mException).isNull();
        int expectedFlags = MS_NOATIME | MS_RDONLY;
        assertThat(testResults.mMountFlags & expectedFlags).isEqualTo(expectedFlags);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.007"})
    @VsrTest(requirements = {"VSR-7.1-001.008"})
    public void bootFailsWithCustomVendorPartitionForPvm() throws Exception {
        assumeProtectedVM();
        File vendorDiskImage =
                new File("/data/local/tmp/cts/microdroid/test_microdroid_vendor_image.img");
        VirtualMachineConfig config = buildVmConfigWithVendor(vendorDiskImage);

        BootResult bootResult = tryBootVmWithConfig(config, "test_boot_with_custom_vendor_pvm");
        assertThat(bootResult.payloadStarted).isFalse();
        assertThat(bootResult.deathReason).isEqualTo(VirtualMachineCallback.STOP_REASON_REBOOT);
    }

    @Test
    @CddTest(requirements = {"3.1/C-0-1"})
    @GmsTest(requirements = {"GMS-VSR-7.1-001.007"})
    @VsrTest(requirements = {"VSR-7.1-001.008"})
    public void creationFailsWithUnsignedVendorPartition() throws Exception {
        File vendorDiskImage =
                new File(
                        "/data/local/tmp/cts/microdroid/test_microdroid_vendor_image_unsigned.img");
        VirtualMachineConfig config = buildVmConfigWithVendor(vendorDiskImage);

        VirtualMachine vm = forceCreateNewVirtualMachine("test_boot_with_unsigned_vendor", config);
        assertThrowsVmExceptionContaining(
                () -> vm.run(), "Failed to extract vendor hashtree digest");
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-004", "GMS-3-7.1-005"})
    public void systemPartitionMountFlags() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_system_mount_flags", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mMountFlags = ts.getMountFlags("/");
                        });

        assertThat(testResults.mException).isNull();
        int expectedFlags = MS_NOATIME | MS_RDONLY;
        assertThat(testResults.mMountFlags & expectedFlags).isEqualTo(expectedFlags);
    }

    @Test
    @GmsTest(requirements = {"GMS-3-7.1-001.002"})
    public void pageSize() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_page_size", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mPageSize = ts.getPageSize();
                        });

        assertThat(testResults.mException).isNull();
        int expectedPageSize = mOs.endsWith("_16k") ? 16384 : 4096;
        assertThat(testResults.mPageSize).isEqualTo(expectedPageSize);
    }

    // This test requires MicrodroidTestApp to have USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION
    // permission. This means that the permission needs to be declared in the AndroidManifest.xml of
    // the MicrodroidTestApp.apk. Which in turns leads microdroid_manager to enable the relaxed
    // rollback protection scheme, which we don't want to be enabled for most of the tests here.
    // For now comment out this test. It will be un-commented (and probably moved to a separate test
    // apk) in a follow-up patch.
    // TODO(ioffe): bring this test back!
    /*
        @Test
        public void libIcuIsLoadable() throws Exception {
            assumeSupportedDevice();
            // This test relies on the test apk having USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION
            // permission.
            grantPermission(USE_RELAXED_MICRODROID_ROLLBACK_PROTECTION_PERMISSION);

            // This test requires additional test apk.
            installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_correct_V5.apk");

            Context otherAppCtx =
                    getContext()
                            .createPackageContext(RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME, 0);

            VirtualMachineConfig config =
                    new VirtualMachineConfig.Builder(otherAppCtx)
                            .setDebugLevel(DEBUG_LEVEL_FULL)
                            .setPayloadBinaryName("MicrodroidTestNativeLibWithLibIcu.so")
                            .setProtectedVm(isProtectedVm())
                            .setOs(os())
                            .build();

            VirtualMachine vm = forceCreateNewVirtualMachine("test_libicu_is_loadable", config);

            TestResults testResults =
                    runVmTestService(
                            TAG,
                            vm,
                            (ts, tr) -> {
                                ts.checkLibIcuIsAccessible();
                            });

            // checkLibIcuIsAccessible will throw an exception if something goes wrong.
            assertThat(testResults.mException).isNull();
        }
    */

    @Test
    public void relaxedRollbackProtectionScheme_apkDoesNotHavePermission_bootFails()
            throws Exception {
        assumeSupportedDevice();

        // This test requires additional test apk.
        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_no_permission.apk");

        Context otherAppCtx =
                getContext()
                        .createPackageContext(
                                RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME, 0);

        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(otherAppCtx)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setPayloadBinaryName("MicrodroidTestNativeLib.so")
                        .setProtectedVm(isProtectedVm())
                        .setOs(os())
                        .build();

        VirtualMachine vm =
                forceCreateNewVirtualMachine(
                        "test_relaxed_rollback_protection_scheme_no_permission", config);
        BootResult bootResult =
                tryBootVm(TAG, "test_relaxed_rollback_protection_scheme_no_permission");
        assertThat(bootResult.deathReason)
                .isEqualTo(
                        VirtualMachineCallback.STOP_REASON_MICRODROID_PAYLOAD_VERIFICATION_FAILED);
    }

    @Test
    public void relaxedRollbackProtectionScheme_apkDoesNotHaveRollbackIndex_bootFails()
            throws Exception {
        assumeSupportedDevice();

        // This test requires additional test apk.
        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_no_rollback_index.apk");

        Context otherAppCtx =
                getContext()
                        .createPackageContext(
                                RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME, 0);

        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(otherAppCtx)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setPayloadBinaryName("MicrodroidTestNativeLib.so")
                        .setProtectedVm(isProtectedVm())
                        .setOs(os())
                        .build();

        VirtualMachine vm =
                forceCreateNewVirtualMachine(
                        "test_relaxed_rollback_protection_scheme_no_rollback_index", config);
        BootResult bootResult =
                tryBootVm(TAG, "test_relaxed_rollback_protection_scheme_no_rollback_index");
        assertThat(bootResult.deathReason)
                .isEqualTo(
                        VirtualMachineCallback.STOP_REASON_MICRODROID_PAYLOAD_VERIFICATION_FAILED);
    }

    @Test
    public void relaxedRollbackProtectionScheme_rollbackVersionDoesNotChange() throws Exception {
        assumeSupportedDevice();
        // Relaxed rollback protection scheme only makes sense if VM updates are supported.
        assumeTrue("Missing Updatable VM support", isUpdatableVmSupported());

        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V6.apk");

        Context testHelperAppCtx =
                getContext()
                        .createPackageContext(
                                RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME, 0);

        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(testHelperAppCtx)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setPayloadBinaryName("MicrodroidTestNativeLib.so")
                        .setProtectedVm(isProtectedVm())
                        .setOs(os())
                        .setEncryptedStorageBytes(1 * 1024 * 1024)
                        .build();

        VirtualMachine vm =
                forceCreateNewVirtualMachine("test_rollback_version_does_not_change", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            ts.writeToFile(
                                    /* content= */ EXAMPLE_STRING,
                                    /* path= */ "/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();

        // Simulate a rollback by installing a downgraded version of the helper apk.
        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V5.apk", "-d");

        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mFileContent = ts.readFromFile("/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();
        assertThat(testResults.mFileContent).isEqualTo(EXAMPLE_STRING);
    }

    @Test
    public void relaxedRollbackProtectionScheme_rollbackVersionChanges() throws Exception {
        assumeSupportedDevice();
        // Relaxed rollback protection scheme only makes sense if VM updates are supported.
        assumeTrue("Missing Updatable VM support", isUpdatableVmSupported());
        assumeProtectedVM();

        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V5.apk");

        Context testHelperAppCtx =
                getContext()
                        .createPackageContext(
                                RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME, 0);

        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(testHelperAppCtx)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setPayloadBinaryName("MicrodroidTestNativeLib.so")
                        .setProtectedVm(isProtectedVm())
                        .setOs(os())
                        .setEncryptedStorageBytes(1 * 1024 * 1024)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_rollback_version_changes", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            ts.writeToFile(
                                    /* content= */ EXAMPLE_STRING,
                                    /* path= */ "/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();

        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V7_inc_rollback_version.apk");

        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mFileContent = ts.readFromFile("/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();
        assertThat(testResults.mFileContent).isEqualTo(EXAMPLE_STRING);

        assertThat(vm.getStatus()).isEqualTo(VirtualMachine.STATUS_STOPPED);

        // Simulate a rollback by installing a downgraded version of the helper apk.
        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V6.apk", "-d");

        // Now pVM shouldn't boot.
        BootResult bootResult = tryBootVm(TAG, vm);
        assertThat(bootResult.deathReason)
                .isEqualTo(VirtualMachineCallback.STOP_REASON_MICRODROID_PAYLOAD_HAS_CHANGED);
    }

    @Test
    public void delayEncryptedStoreSetup() throws Exception {
        assumeSupportedDevice();

        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_delay_enc_store.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(1_000_000)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_delay_enc_store", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            // This call will also check that encrypted store is not mounted.
                            ts.requestEncryptedStoreSetup();
                            ts.writeToFile("Hello!", "/mnt/encryptedstore/file.txt");
                            tr.mFileContent = ts.readFromFile("/mnt/encryptedstore/file.txt");
                        });
        testResults.assertNoException();
        assertThat(testResults.mFileContent).isEqualTo("Hello!");
    }

    @Test
    public void encryptedStoreKekOnCe_vmIsOnDe() throws Exception {
        assumeSupportedDevice();
        installApp("MicrodroidTestHelperEncStoreKEKOnCE_V6.apk");
        Context ctx = getContext().createDeviceProtectedStorageContext();
        encryptedStoreKEKTest(ctx, "test_vm_enc_store_kek_on_ce_vm_on_de");
    }

    @Test
    public void encryptedStoreKekOnCe_vmIsOnCe() throws Exception {
        assumeSupportedDevice();
        installApp("MicrodroidTestHelperEncStoreKEKOnCE_V6.apk");
        Context ctx = getContext().createCredentialProtectedStorageContext();
        encryptedStoreKEKTest(ctx, "test_vm_enc_store_kek_on_ce_vm_on_ce");
    }

    private void encryptedStoreKEKTest(Context context, String testName) throws Exception {
        Context testHelperAppCtx =
                getContext().createPackageContext(ENCRYPTED_STORE_KEK_ON_CE_TEST_PACKAGE_NAME, 0);
        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(testHelperAppCtx)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setPayloadBinaryName("MicrodroidTestNativeLib.so")
                        .setProtectedVm(isProtectedVm())
                        .setOs(os())
                        .setEncryptedStorageBytes(1 * 1024 * 1024)
                        .setMemoryBytes(minMemoryRequired())
                        .build();

        VirtualMachineManager vmm = context.getSystemService(VirtualMachineManager.class);
        try {
            if (vmm.get(testName) != null) {
                vmm.delete(testName);
            }
        } catch (VirtualMachineException e) {
            // VirtualMachineManager#get might throw VirtualMachineException, which means that VM
            // exist but didn't load successfully, delete it.
            vmm.delete(testName);
        }
        VirtualMachine vm = vmm.create(testName, config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            // This call will also check that encrypted store is not mounted.
                            ts.requestEncryptedStoreSetup();
                            ts.writeToFile("Hello!", "/mnt/encryptedstore/file.txt");
                            tr.mFileContent = ts.readFromFile("/mnt/encryptedstore/file.txt");
                        });
        testResults.assertNoException();
        assertThat(testResults.mFileContent).isEqualTo("Hello!");

        testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            // This call will also check that encrypted store is not mounted.
                            ts.requestEncryptedStoreSetup();
                            tr.mFileContent = ts.readFromFile("/mnt/encryptedstore/file.txt");
                        });
        testResults.assertNoException();
        assertThat(testResults.mFileContent).isEqualTo("Hello!");

        // Check that VM files are created in the expected location.
        File vmDir = new File(new File(context.getDataDir(), "vm"), testName);
        assertWithMessage(vmDir.getAbsolutePath() + " does not exist")
                .that(vmDir.exists())
                .isTrue();
        File instanceImg = new File(vmDir, "instance.img");
        assertWithMessage(instanceImg.getAbsolutePath() + " does not exist")
                .that(instanceImg.exists())
                .isTrue();
        File encStoreImg = new File(vmDir, "storage.img");
        assertWithMessage(encStoreImg.getAbsolutePath() + " doest no exist")
                .that(encStoreImg.exists())
                .isTrue();
        // The KEK file is always stored on CE directory.
        Context ceContext = context.createCredentialProtectedStorageContext();
        File ceVmDir = new File(new File(ceContext.getDataDir(), "vm"), testName);
        assertWithMessage(ceVmDir.getAbsolutePath() + " does not exist")
                .that(ceVmDir.exists())
                .isTrue();
        File kek = new File(ceVmDir, "encrypted_store_kek.bin");
        assertWithMessage(kek.getAbsolutePath() + " does not exist").that(kek.exists()).isTrue();
    }

    /**
     * Verifies that changing the encrypted store is an incompatible config change.
     *
     * <p>NOTE: this is a separate test case from {@link #compatibleConfigTests} because we don't
     * expose set/get encrypted store mode APIs as @SystemApis yet.
     *
     * @see VirtualMachineConfig#isCompatibleWith
     */
    @Test
    public void changingEncryptedStoreModeOnUpdateTests() throws Exception {
        assumeSupportedDevice();

        installApp("MicrodroidTestHelperEncStoreKEKOnCE_V5.apk");
        Context context =
                getContext()
                        .createPackageContext(ENCRYPTED_STORE_KEK_ON_CE_TEST_PACKAGE_NAME, 0)
                        .createDeviceProtectedStorageContext();

        VirtualMachineConfig config1 =
                new VirtualMachineConfig.Builder(context)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setPayloadBinaryName("MicrodroidTestNativeLib.so")
                        .setProtectedVm(isProtectedVm())
                        .setOs(os())
                        .setEncryptedStorageBytes(1 * 1024 * 1024)
                        .setMemoryBytes(minMemoryRequired())
                        .build();

        // Simulate update that changes the encrypted store mode.
        installApp("MicrodroidTestHelperEncStoreKEKOnCE_V6.apk");

        // The encrypted store mode is read from the AndroidManifest.xml of the app represented by
        // the context object passed to the builder c-tor. The V6 update installed above has the
        // android.system.virtualmachine.ENCRYPTED_STORE_MODE property set to 1.
        VirtualMachineConfig config2 =
                new VirtualMachineConfig.Builder(context)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setPayloadBinaryName("MicrodroidTestNativeLib.so")
                        .setProtectedVm(isProtectedVm())
                        .setOs(os())
                        .setEncryptedStorageBytes(1 * 1024 * 1024)
                        .setMemoryBytes(minMemoryRequired())
                        .build();

        assertThat(config1.isCompatibleWith(config2)).isFalse();
    }

    /**
     * Tests the end-to-end flow of mounting encrypted assets within the VM.
     *
     * <p>This test verifies that the payload can request to mount an encrypted image bundled in the
     * APK assets, and that the content of the decrypted filesystem is accessible at the expected
     * mount point.
     */
    @Test
    public void mountEncryptedAssets() throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so").build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_encrypted_asset", config);

        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mEncryptedAssetsPath =
                                    ts.mountEncryptedAssets("/mnt/apk/assets/encrypted_assets.bin");
                            tr.mFileContent =
                                    ts.readFromFile(tr.mEncryptedAssetsPath + "/file.txt");
                        });
        testResults.assertNoException();

        assertThat(testResults.mEncryptedAssetsPath).isEqualTo("/mnt/encrypted_assets");
        assertThat(testResults.mFileContent).isEqualTo("Top secret in encrypted assets!\n");
    }

    private static class VmShareServiceConnection implements ServiceConnection {

        private final CountDownLatch mLatch = new CountDownLatch(1);

        private IVmShareTestService mVmShareTestService;

        @Override
        public void onServiceConnected(ComponentName name, IBinder service) {
            mVmShareTestService = IVmShareTestService.Stub.asInterface(service);
            mLatch.countDown();
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {}

        private IVmShareTestService waitForService() throws Exception {
            if (!mLatch.await(1, TimeUnit.MINUTES)) {
                return null;
            }
            return mVmShareTestService;
        }
    }

    @Test
    public void concurrentVms() throws Exception {
        final long vmSize = minMemoryRequired();
        final int numVMs = 8;
        final long availableMem = getAvailableMemory();

        // Let's not use more than half of the available memory
        assume().withMessage("Available memory (" + availableMem + " bytes) too small")
                .that((numVMs * vmSize) <= (availableMem / 2))
                .isTrue();

        VirtualMachine[] vms = new VirtualMachine[numVMs];
        try {
            for (int i = 0; i < numVMs; i++) {
                VirtualMachineConfig config =
                        newVmConfigBuilderWithPayloadBinary("MicrodroidIdleNativeLib.so")
                                .setDebugLevel(DEBUG_LEVEL_NONE)
                                .setMemoryBytes(vmSize)
                                .build();

                vms[i] = forceCreateNewVirtualMachine("test_concurrent_vms_" + i, config);
                vms[i].run();
            }

            for (VirtualMachine vm : vms) {
                assertThat(vm.getStatus()).isEqualTo(VirtualMachine.STATUS_RUNNING);
            }

        } finally {
            // Ensure that VMs are all stopped. Otherwise we may try to reuse some of these for
            // another run of this test with different parameters.
            for (VirtualMachine vm : vms) {
                if (vm != null) {
                    vm.close();
                }
            }
        }
    }

    @Test
    public void stopAndRestart() throws Exception {
        String vmName = "stop_and_restart";
        ExecutorService executor = Executors.newSingleThreadExecutor();
        VirtualMachineConfig.Builder builder =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setCpuTopology(CPU_TOPOLOGY_ONE_CPU);
        VirtualMachineConfig config = builder.build();
        try (VirtualMachine vm = forceCreateNewVirtualMachine(vmName, config)) {

            // Start a VM and kill it abrubtly.
            SimpleVirtualMachineCallback cb1 = new SimpleVirtualMachineCallback();
            vm.setCallback(executor, cb1);
            vm.run();
            cb1.started.await();
            kill(TAG, "crosvm_" + vmName);
            vm.close();
            cb1.stopped.await();

            // Re-start the same VM, but with a different callback.
            SimpleVirtualMachineCallback cb2 = new SimpleVirtualMachineCallback();
            vm.setCallback(executor, cb2);
            vm.run();
            cb2.started.await();

            // Stopping of the first VM shouldn't be notified to the new callback.
            assertThat(cb2.stopped.getCount()).isEqualTo(1);
            vm.close();
            cb2.stopped.await();
        }
    }

    @Test
    public void vmListDoesNotShowDeadVirtualMachines() throws Exception {
        UiAutomation uia = InstrumentationRegistry.getInstrumentation().getUiAutomation();

        final int numVMs = 5;

        List<VirtualMachine> vms = new ArrayList<>();
        ExecutorService executor = Executors.newSingleThreadExecutor();
        for (int i = 0; i < numVMs; i++) {
            String vmName = "vmList_" + i;
            VirtualMachineConfig.Builder builder =
                    newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                            .setMemoryBytes(minMemoryRequired())
                            .setDebugLevel(DEBUG_LEVEL_FULL)
                            .setCpuTopology(CPU_TOPOLOGY_ONE_CPU);
            VirtualMachineConfig config = builder.build();
            VirtualMachine vm = forceCreateNewVirtualMachine(vmName, config);
            SimpleVirtualMachineCallback cb = new SimpleVirtualMachineCallback();
            vm.setCallback(executor, cb);
            vm.run();
            cb.started.await();
            vms.add(vm);

            assertThat(runInShell(TAG, uia, "vm list")).contains("name: \"" + vmName + "\"");
        }

        for (VirtualMachine vm : vms) {
            kill(TAG, "crosvm_" + vm.getName());
        }
        // It may take some time for the crosvm processes to get the SIGKILL signal
        Thread.sleep(5000);

        String vmList = runInShell(TAG, uia, "vm list").trim();
        for (VirtualMachine vm : vms) {
            assertThat(vmList).doesNotContain("name: \"" + vm.getName() + "\"");
        }

        for (VirtualMachine vm : vms) {
            vm.close();
        }
    }

    @Test
    public void vmNameIsHostname() throws Exception {
        assumeSupportedDevice();
        assumeTrue("MultiTenancy feature not supported", isFeatureMultiTenantSupported());
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        // Run a non-debuggable pVM to also cover pvmfw filtering logic in this
                        // test.
                        .setDebugLevel(DEBUG_LEVEL_NONE)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm-name42", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        vm,
                        (ts, tr) -> {
                            tr.mHostname = ts.getHostname();
                        });
        testResults.assertNoException();
        assertThat(testResults.mHostname).isEqualTo("test_vm-name42");
    }

    // Verify Microdroid fails to boot immediately when presented with a stale disk image
    @Test
    public void staleEncryptedstoreDetection() throws Exception {
        assumeSupportedDevice();

        // Create and run misc_vm.
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine misc_vm = forceCreateNewVirtualMachine("misc_vm", config);
        TestResults testResults =
                runVmTestService(
                        TAG,
                        misc_vm,
                        (ts, tr) -> {
                            ts.writeToFile(
                                    /* content= */ EXAMPLE_STRING,
                                    /* path= */ "/mnt/encryptedstore/test_file");
                        });
        testResults.assertNoException();

        // Create vm_under_test
        VirtualMachine vm_under_test = forceCreateNewVirtualMachine("vm_under_test", config);
        // Plug the disk of `misc_vm` into the newly created `vm_under_test`
        Files.copy(
                getVmFile("misc_vm", "storage.img").toPath(),
                getVmFile("vm_under_test", "storage.img").toPath(),
                REPLACE_EXISTING);

        // Rerun `vm_under_test` with stale disk
        var onPayloadReadyExecuted = new CompletableFuture<Boolean>();
        var onErrorExecuted = new CompletableFuture<Boolean>();
        var errorMessage = new CompletableFuture<String>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        onPayloadReadyExecuted.complete(true);
                        super.onPayloadReady(vm);
                    }

                    @Override
                    public void onError(VirtualMachine vm, int errorCode, String message) {
                        onErrorExecuted.complete(true);
                        errorMessage.complete(message);
                        super.onError(vm, errorCode, message);
                    }
                };
        listener.runToFinish(TAG, vm_under_test);

        assertThat(onPayloadReadyExecuted.getNow(false)).isFalse();
        assertThat(onErrorExecuted.getNow(false)).isTrue();
        assertThat(errorMessage.getNow("")).contains("Detected stale encryptedstore");
    }

    private VirtualMachineDescriptor toParcelFromParcel(VirtualMachineDescriptor descriptor) {
        Parcel parcel = Parcel.obtain();
        descriptor.writeToParcel(parcel, 0);
        parcel.setDataPosition(0);
        return VirtualMachineDescriptor.CREATOR.createFromParcel(parcel);
    }

    private void assertFileContentsAreEqualInTwoVms(String fileName, String vmName1, String vmName2)
            throws IOException {
        File file1 = getVmFile(vmName1, fileName);
        File file2 = getVmFile(vmName2, fileName);
        try (FileInputStream input1 = new FileInputStream(file1);
                FileInputStream input2 = new FileInputStream(file2)) {
            assertThat(Arrays.equals(input1.readAllBytes(), input2.readAllBytes())).isTrue();
        }
    }

    private File getVmFile(String vmName, String fileName) {
        Context context = getContext();
        Path filePath = Paths.get(context.getDataDir().getPath(), "vm", vmName, fileName);
        return filePath.toFile();
    }

    /**
     * Generates a list of valid UIDs for tenants, starting from {@code FIRST_TENANT_UID}.
     *
     * @param numberOfUids The number of UIDs to generate in the list.
     */
    private List<Integer> generateValidUidsForTenants(int numberOfUids) {
        List<Integer> validUids = new ArrayList<>();
        for (int i = 0; i < numberOfUids; i++) {
            validUids.add(FIRST_TENANT_UID + i);
        }
        return validUids;
    }

    private void assertThrowsVmException(ThrowingRunnable runnable) {
        assertThrows(VirtualMachineException.class, runnable);
    }

    private void assertThrowsVmExceptionContaining(
            ThrowingRunnable runnable, String expectedContents) {
        Exception e = assertThrows(VirtualMachineException.class, runnable);
        assertThat(e).hasMessageThat().contains(expectedContents);
    }

    private void assertThrowsVmException(ThrowingRunnable runnable, int code, String msg) {
        VirtualMachineException e = assertThrows(VirtualMachineException.class, runnable);
        if (com.android.system.virtualmachine.flags.Flags.virtualmachineexceptionCode()) {
            assertThat(e.getCode()).isEqualTo(code);
        } else if (msg != null) {
            assertThat(e).hasMessageThat().contains(msg);
        }
    }

    private void installApp(String apkName, String... additionalArgs) throws Exception {
        String apkFile = new File("/data/local/tmp/cts/microdroid/", apkName).getAbsolutePath();
        UiAutomation uai = InstrumentationRegistry.getInstrumentation().getUiAutomation();
        Log.i(TAG, "Installing apk " + apkFile);
        // We read the output of the shell command not only to see if it succeeds, but also to make
        // sure that the installation finishes. This avoids a race condition when test tries to
        // create a context of the installed package before the installation finished.
        String installCmd = "pm install " + String.join(" ", additionalArgs) + " " + apkFile;
        try (ParcelFileDescriptor pfd = uai.executeShellCommand(installCmd)) {
            try (InputStream is = new FileInputStream(pfd.getFileDescriptor())) {
                try (BufferedReader br = new BufferedReader(new InputStreamReader(is))) {
                    String line;
                    while ((line = br.readLine()) != null) {
                        Log.i(TAG, line);
                    }
                }
            }
        }
    }

    private void uninstallApp(String packageName) {
        Log.i(TAG, "Uninstalling package " + packageName);
        UiAutomation uai = InstrumentationRegistry.getInstrumentation().getUiAutomation();
        try (ParcelFileDescriptor pfd = uai.executeShellCommand("pm uninstall " + packageName)) {
            try (InputStream is = new FileInputStream(pfd.getFileDescriptor())) {
                try (BufferedReader br = new BufferedReader(new InputStreamReader(is))) {
                    String line;
                    while ((line = br.readLine()) != null) {
                        Log.i(TAG, line);
                    }
                }
            }
        } catch (Exception e) {
            Log.e(TAG, "Failed to uninstall " + packageName, e);
        }
    }
}
