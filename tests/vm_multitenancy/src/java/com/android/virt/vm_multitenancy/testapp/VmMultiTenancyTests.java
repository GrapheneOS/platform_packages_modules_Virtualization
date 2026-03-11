/*
 * Copyright (C) 2024 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may
 * obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package com.android.virt.vm_multitenancy.testapp;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.Truth.assertWithMessage;
import static com.google.common.truth.TruthJUnit.assume;

import static org.junit.Assume.assumeTrue;

import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineCallback;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineException;

import com.android.microdroid.test.device.MicrodroidDeviceTestBase;
import com.android.microdroid.testservice.ITestService;
import com.android.virt.vm_attestation.testservice.IAttestationService.AttestationStatus;
import com.android.virt.vm_attestation.testservice.IAttestationService.SigningResult;

import org.junit.After;
import org.junit.Before;
import org.junit.Ignore;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.List;
import java.util.concurrent.CompletableFuture;

@RunWith(Parameterized.class)
public class VmMultiTenancyTests extends MicrodroidDeviceTestBase {
    private static final String TAG = "VmMultiTenancyTests";
    private static final String MICRODROID_TEST_APP = "com.android.microdroid.test";
    private static final String TEST_TENANT_APK_NAME = "apk:com.android.microdroid.test";
    private static final String VM_ATTESTATION_MESSAGE = "Hello RKP from AVF!";
    private static final int ENCRYPTED_STORAGE_BYTES = 4_000_000;
    private static final String EXAMPLE_STRING = "Literally any string!! :)";
    private static final int MICRODROID_TENANT_UID_RANGE_START = 10000;
    private static final int MICRODROID_TENANT_UID_RANGE_END = 65534;

    private static final String RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME =
            "com.android.microdroid.test_relaxed_rollback_protection_scheme";
    private static final String TEST_ALTERNATE_APP_PACKAGE_NAME =
            "com.android.microdroid.test_alternate_tenant";
    private static final String TEST_APP_PACKAGE_NAME = "com.android.microdroid.test";

    @Parameterized.Parameter(0)
    public boolean mProtectedVm;

    @Parameterized.Parameter(1)
    public String mOs;

    @Parameterized.Parameters(name = "protectedVm={0},os={1}")
    public static Collection<Object[]> params() {
        List<Object[]> ret = new ArrayList<>();
        for (String os : SUPPORTED_OSES) {
            ret.add(new Object[] {true /* protectedVm */, os});
            ret.add(new Object[] {false /* protectedVm */, os});
        }
        return ret;
    }

    @Before
    public void setup() {
        prepareTestSetup(mProtectedVm, mOs);
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
    }

    @After
    public void tearDown() {
        deleteAllExistingVMsByApp();
        revokePermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        uninstallApp(RELAXED_ROLLBACK_PROTECTION_SCHEME_TEST_PACKAGE_NAME);
    }

    private void assumeAdvanceMultiTenancySupport() throws Exception {
        assumeSupportedDevice();
        assumeTrue(
                "AVF Advance Multi-tenancy feature not enabled",
                isFeatureEnabled("com.android.kvm.ADVANCE_MULTITENANCY"));
    }

    @Test
    public void vmAttestationWithMultipleTenantsWhenRemoteAttestationIsNotSupported()
            throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        assume().withMessage(
                        "This test does not apply to a device that supports Remote Attestation")
                .that(isRemoteAttestationSupported())
                .isFalse();
        assumeAdvanceMultiTenancySupport();
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_tenant_attestation.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm =
                forceCreateNewVirtualMachine(
                        "attestation_not_supported_with_multitenant_payload", config);
        byte[] challenge = new byte[32];
        Arrays.fill(challenge, (byte) 0xcc);

        // Act.
        SigningResult signingResult =
                runVmAttestationService(TAG, vm, challenge, VM_ATTESTATION_MESSAGE.getBytes());

        // Assert.
        assertThat(signingResult.status).isEqualTo(AttestationStatus.ERROR_UNSUPPORTED);
    }

    @Test
    public void vmAttestationWithMultipleTenantsSucceedsWithInternet() throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        assumeAdvanceMultiTenancySupport();
        assumeVmAttestationSupportedWithInternet();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_tenant_attestation.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm =
                forceCreateNewVirtualMachine("attestation_with_multitenant_payload", config);

        runAndVerifyVmAttestationSucceeds(
                vm, TEST_TENANT_APK_NAME, MICRODROID_TEST_APP, VM_ATTESTATION_MESSAGE);
    }

    @Test
    public void multipleTenantServices() throws Exception {
        assumeAdvanceMultiTenancySupport();

        String configFile = "assets/vm_config_test_multi_tenants.json";
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(configFile)
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
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
        android.os.Trace.beginSection("multitenant_vm");
        try {
            listener.runToFinish(TAG, vm);
        } finally {
            android.os.Trace.endSection();
        }
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
    public void multipleTenantUids() throws Exception {
        assumeAdvanceMultiTenancySupport();

        String configFile = "assets/vm_config_test_multi_tenants.json";
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(configFile)
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        final int NUMBER_OF_TENANTS_IN_CONFIG = 2;
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
        assumeAdvanceMultiTenancySupport();
        installApp("MicrodroidTestHelperAppAlternateTenant.apk");
        String configFile = "assets/vm_config_test_multi_tenants.json";
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(configFile)
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
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
                .isEqualTo("u:r:aiseal_agent_tenant:s0");
    }

    @Test
    public void multiTenantEncryptedStoragePath() throws Exception {
        assumeAdvanceMultiTenancySupport();

        String configFile = "assets/vm_config_test_multi_tenants.json";
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(configFile)
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
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
    public void multiTenantApkContentsPath() throws Exception {
        assumeAdvanceMultiTenancySupport();

        String configFile = "assets/vm_config_test_multi_tenants.json";
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(configFile)
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm =
                forceCreateNewVirtualMachine("test_vm_tenant_apk_contents_path", config);
        CompletableFuture<String> tenant1ApkContentsPath = new CompletableFuture<>();
        CompletableFuture<String> tenant2ApkContentsPath = new CompletableFuture<>();
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
                            tenant1ApkContentsPath.complete(tsOnAPort.getApkContentsPath());

                            ITestService tsOnAlternatePort =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.ALTERNATE_PORT));
                            tenant2ApkContentsPath.complete(tsOnAlternatePort.getApkContentsPath());

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
                                + " test_vm_tenant_apk_contents_path's onPayloadReady"
                                + " callback")
                .that(exception.getNow(null))
                .isNull();

        assertWithMessage("Tenant 1 apk contents path should be specific to the tenant")
                .that(tenant1ApkContentsPath.getNow(null))
                .isEqualTo("/mnt/apk/" + TEST_APP_PACKAGE_NAME);
        assertWithMessage("Tenant 2 apk contents path should be specific to the tenant")
                .that(tenant2ApkContentsPath.getNow(null))
                .isEqualTo("/mnt/apk/" + TEST_ALTERNATE_APP_PACKAGE_NAME);
        assertThat(exitCodeFuture.getNow(500)).isEqualTo(0);
    }


    @Test
    public void addingMoretenantsIsSupported() throws Exception {
        assumeAdvanceMultiTenancySupport();

        // First run with a single tenant.
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_single_tenant.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_add_tenant", config);
        CompletableFuture<String> result = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted != PASS")
                .that(result.getNow(null))
                .isEqualTo("PASS");

        // Re-run the VM with more tenants
        String configFile = "assets/vm_config_test_multi_tenants.json";
        config =
                newVmConfigBuilderWithPayloadConfig(configFile)
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
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
    public void testEncryptedStorageIsPersistentOnAddTenant() throws Exception {
        assumeAdvanceMultiTenancySupport();

        // First run with a single tenant.
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_single_tenant.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
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
        String configFile = "assets/vm_config_test_multi_tenants.json";
        config =
                newVmConfigBuilderWithPayloadConfig(configFile)
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
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
    public void interTenantCommunication() throws Exception {
        assumeAdvanceMultiTenancySupport();

        assumeTrue(isApiLevel37Supported());

        String configFile = "assets/vm_config_test_multi_tenants.json";
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(configFile)
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_inter_tenant_comm", config);
        CompletableFuture<String> dataReceivedByClient = new CompletableFuture<>();
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Integer> exitCodeFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        try {
                            ITestService server =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            server.startUdsServerWithData(EXAMPLE_STRING);

                            ITestService client =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.ALTERNATE_PORT));
                            dataReceivedByClient.complete(client.startUdsClientAndGetData());

                            server.quit();
                            client.quit();
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
                        "Unexpected exception while running test_vm_inter_tenant_comm's"
                                + " onPayloadReady callback")
                .that(exception.getNow(null))
                .isNull();

        assertThat(exitCodeFuture.getNow(500)).isEqualTo(0);

        assertWithMessage("There is a mismatch in data received by client")
                .that(dataReceivedByClient.getNow(null))
                .isEqualTo(EXAMPLE_STRING);
    }

    @Test
    public void duplicateTenantsAreRejected() throws Exception {
        assumeAdvanceMultiTenancySupport();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_invalid_duplicate_tenants.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setEncryptedStorageBytes(ENCRYPTED_STORAGE_BYTES)
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_duplicate_tenants", config);
        CompletableFuture<String> res = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(res.getNow(null))
                .isNull();
    }

    @Test
    public void bootFailsWhenMinVersionIsMissing() throws Exception {
        assumeAdvanceMultiTenancySupport();
        assertBootFailsWithConfig(
                "assets/vm_config_missing_min_version.json",
                VirtualMachineException.CODE_PAYLOAD_CONFIG_MALFORMED,
                "missing field `min_version`");
    }

    @Test
    public void bootFailsWhenExpectedAuthorityIsMissing() throws Exception {
        assumeAdvanceMultiTenancySupport();
        assertBootFailsWithConfig(
                "assets/vm_config_missing_expected_authority.json",
                VirtualMachineException.CODE_PAYLOAD_CONFIG_MALFORMED,
                "missing field `expected_authority`");
    }

    @Test
    public void bootFailsWhenTenantUidIsMissing() throws Exception {
        assumeAdvanceMultiTenancySupport();
        assertBootFailsWithConfig(
                "assets/vm_config_tenant_no_uid.json",
                VirtualMachineException.CODE_PAYLOAD_CONFIG_MALFORMED,
                "missing field `uid`");
    }

    @Test
    public void bootFailsWhenTenantUidIsTooLow() throws Exception {
        assumeAdvanceMultiTenancySupport();
        assertBootFailsWithConfig(
                "assets/vm_config_tenant_uid_too_low.json",
                VirtualMachineException.CODE_PAYLOAD_CONFIG_MALFORMED,
                "Tenant UID 9999 is invalid. It must be in range ["
                        + MICRODROID_TENANT_UID_RANGE_START
                        + ", "
                        + MICRODROID_TENANT_UID_RANGE_END
                        + "]");
    }

    @Test
    public void bootFailsWhenTenantUidIsTooHigh() throws Exception {
        assumeAdvanceMultiTenancySupport();
        assertBootFailsWithConfig(
                "assets/vm_config_tenant_uid_too_high.json",
                VirtualMachineException.CODE_PAYLOAD_CONFIG_MALFORMED,
                "Tenant UID 65535 is invalid. It must be in range ["
                        + MICRODROID_TENANT_UID_RANGE_START
                        + ", "
                        + MICRODROID_TENANT_UID_RANGE_END
                        + "]");
    }

    @Test
    public void bootFailsWhenTenantUidIsNegative() throws Exception {
        assumeAdvanceMultiTenancySupport();
        assertBootFailsWithConfig(
                "assets/vm_config_tenant_uid_negative.json",
                VirtualMachineException.CODE_PAYLOAD_CONFIG_MALFORMED,
                "Tenant UID -1 is invalid. It must be in range ["
                        + MICRODROID_TENANT_UID_RANGE_START
                        + ", "
                        + MICRODROID_TENANT_UID_RANGE_END
                        + "]");
    }

    @Test
    public void bootFailsWhenTenantUidIsDuplicate() throws Exception {
        assumeAdvanceMultiTenancySupport();
        assertBootFailsWithConfig(
                "assets/vm_config_tenant_uid_duplicate.json",
                VirtualMachineException.CODE_PAYLOAD_CONFIG_MALFORMED,
                "Duplicate tenant UID found: 10000");
    }

    private void assertBootFailsWithConfig(
            String configPath, int expectedErrorCode, String expectedErrorMessage)
            throws Exception {

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(configPath)
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();

        assertThrowsVmException(
                () -> tryBootVmWithConfig(config, "test_vm_boot_failure"),
                expectedErrorCode,
                expectedErrorMessage);
    }

    @Test
    public void invalidTenantApkAuthority() throws Exception {
        assumeAdvanceMultiTenancySupport();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_invalid_tenant_auth.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("tenant_with_different_cert", config);
        CompletableFuture<String> res = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(res.getNow(null))
                .isNull();
    }

    @Test
    public void invalidTenantApexAuthority() throws Exception {
        assumeAdvanceMultiTenancySupport();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_invalid_tenant_apex_auth.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("tenant_with_different_cert_fail", config);
        CompletableFuture<String> res = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(res.getNow(null))
                .isNull();
    }

    @Test
    public void invalidTenantApexVersion() throws Exception {
        assumeAdvanceMultiTenancySupport();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_invalid_tenant_apex_version.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("tenant_with_lower_version_fail", config);
        CompletableFuture<String> res = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(res.getNow(null))
                .isNull();
    }

    @Test
    public void invalidTenantRollbackIndex() throws Exception {
        assumeAdvanceMultiTenancySupport();

        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V6.apk", "-d");
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_tenant_rollback_index.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("tenant_rollback_index_1", config);
        CompletableFuture<String> result_rb_1 = readTenantPackagesMounted(vm);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted should be null")
                .that(result_rb_1.getNow(null))
                .isNull();

        installApp("MicrodroidTestHelperAppRelaxedRollbackProtection_V7_inc_rollback_version.apk");
        VirtualMachine vm2 = forceCreateNewVirtualMachine("tenant_rollback_index_2", config);
        CompletableFuture<String> result_rb_2 = readTenantPackagesMounted(vm2);
        assertWithMessage("debug.microdroid.test.tenant_packages_mounted != PASS")
                .that(result_rb_2.getNow(null))
                .isEqualTo("PASS");
    }

    // TODO(b/441899073) Microdroid manager does not yet persist or validate the tenants data
    // against instance spec.
    @Ignore
    @Test
    public void multiTenantInstanceSpecRollbackTest() throws Exception {
        assumeAdvanceMultiTenancySupport();

        // MicrodroidTestHelperAppRelaxedRollbackProtection_V7*.apk has rollback_index:2
        installApp(
                "MicrodroidTestHelperAppRelaxedRollbackProtection_V7_inc_rollback_version.apk",
                "-d");

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig(
                                "assets/vm_config_tenant_rollback_index_instance_spec.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
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

    @Test
    public void bootFailsWhenBothMainTaskAndTenantTaskArePresent() throws Exception {
        assumeAdvanceMultiTenancySupport();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config_main_and_tenant_tasks.json")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_FULL)
                        .build();

        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm_main_and_tenant_tasks", config);
        BootResult bootResult = tryBootVm(TAG, vm);
        assertThat(bootResult.payloadStarted).isFalse();
        assertThat(bootResult.deathReason)
                .isEqualTo(VirtualMachineCallback.STOP_REASON_MICRODROID_INVALID_PAYLOAD_CONFIG);
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
                        "Unexpected exception while running readTenantPackagesMounted")
                .that(exception.getNow(null))
                .isNull();
        return prop;
    }

    /**
     * Generates a list of valid UIDs for tenants, starting from {@code
     * MICRODROID_TENANT_UID_RANGE_START}
     *
     * @param numberOfUids The number of UIDs to generate in the list.
     */
    private List<Integer> generateValidUidsForTenants(int numberOfUids) {
        List<Integer> validUids = new ArrayList<>();
        for (int i = 0; i < numberOfUids; i++) {
            validUids.add(MICRODROID_TENANT_UID_RANGE_START + i);
        }
        return validUids;
    }
}
