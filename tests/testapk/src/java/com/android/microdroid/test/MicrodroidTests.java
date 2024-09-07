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

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.Truth.assertWithMessage;
import static com.google.common.truth.TruthJUnit.assume;

import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;
import static org.junit.Assume.assumeFalse;
import static org.junit.Assume.assumeTrue;

import static java.nio.file.StandardCopyOption.REPLACE_EXISTING;
import static java.util.stream.Collectors.toList;

import android.app.ActivityManager;
import android.app.Instrumentation;
import android.app.UiAutomation;
import android.content.ComponentName;
import android.content.Context;
import android.content.ContextWrapper;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Build;
import android.os.IBinder;
import android.os.Parcel;
import android.os.ParcelFileDescriptor;
import android.os.ParcelFileDescriptor.AutoCloseInputStream;
import android.os.ParcelFileDescriptor.AutoCloseOutputStream;
import android.os.SystemProperties;
import android.system.OsConstants;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineCallback;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineDescriptor;
import android.system.virtualmachine.VirtualMachineException;
import android.system.virtualmachine.VirtualMachineManager;

import androidx.test.platform.app.InstrumentationRegistry;

import com.android.compatibility.common.util.CddTest;
import com.android.compatibility.common.util.VsrTest;
import com.android.microdroid.test.device.MicrodroidDeviceTestBase;
import com.android.microdroid.test.vmshare.IVmShareTestService;
import com.android.microdroid.testservice.IAppCallback;
import com.android.microdroid.testservice.ITestService;
import com.android.microdroid.testservice.IVmCallback;
import com.android.virt.vm_attestation.testservice.IAttestationService.AttestationStatus;
import com.android.virt.vm_attestation.testservice.IAttestationService.SigningResult;
import com.android.virt.vm_attestation.util.X509Utils;

import co.nstant.in.cbor.CborDecoder;
import co.nstant.in.cbor.model.Array;
import co.nstant.in.cbor.model.DataItem;
import co.nstant.in.cbor.model.MajorType;

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
import java.io.ByteArrayInputStream;
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
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import java.util.stream.Stream;

@RunWith(Parameterized.class)
public class MicrodroidTests extends MicrodroidDeviceTestBase {
    private static final String TAG = "MicrodroidTests";
    private static final String TEST_APP_PACKAGE_NAME = "com.android.microdroid.test";
    private static final String VM_ATTESTATION_PAYLOAD_PATH = "libvm_attestation_test_payload.so";
    private static final String VM_ATTESTATION_MESSAGE = "Hello RKP from AVF!";
    private static final int ENCRYPTED_STORAGE_BYTES = 4_000_000;

    @Rule public Timeout globalTimeout = Timeout.seconds(300);

    @Parameterized.Parameters(name = "protectedVm={0},gki={1}")
    public static Collection<Object[]> params() {
        List<Object[]> ret = new ArrayList<>();
        ret.add(new Object[] {true /* protectedVm */, null /* use microdroid kernel */});
        ret.add(new Object[] {false /* protectedVm */, null /* use microdroid kernel */});
        // TODO(b/302465542): run only the latest GKI on presubmit to reduce running time
        for (String gki : SUPPORTED_GKI_VERSIONS) {
            ret.add(new Object[] {true /* protectedVm */, gki});
            ret.add(new Object[] {false /* protectedVm */, gki});
        }
        return ret;
    }

    @Parameterized.Parameter(0)
    public boolean mProtectedVm;

    @Parameterized.Parameter(1)
    public String mGki;

    @Before
    public void setup() {
        prepareTestSetup(mProtectedVm, mGki);
        if (mGki != null) {
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
        revokePermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
    }

    private static final long ONE_MEBI = 1024 * 1024;

    private static final long MIN_MEM_ARM64 = 170 * ONE_MEBI;
    private static final long MIN_MEM_X86_64 = 196 * ONE_MEBI;
    private static final String EXAMPLE_STRING = "Literally any string!! :)";

    private static final String VM_SHARE_APP_PACKAGE_NAME = "com.android.microdroid.vmshare_app";

    private void createAndConnectToVmHelper(int cpuTopology) throws Exception {
        assumeSupportedDevice();

        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setMemoryBytes(minMemoryRequired())
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setCpuTopology(cpuTopology)
                        .build();
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
    public void createAndConnectToVm() throws Exception {
        createAndConnectToVmHelper(CPU_TOPOLOGY_ONE_CPU);
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
    public void createAndConnectToVm_HostCpuTopology() throws Exception {
        createAndConnectToVmHelper(CPU_TOPOLOGY_MATCH_HOST);
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
    public void vmAttestationWhenRemoteAttestationIsNotSupported() throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        assumeFeatureEnabled(VirtualMachineManager.FEATURE_REMOTE_ATTESTATION);
        assume().withMessage(
                        "This test does not apply to a device that supports Remote Attestation")
                .that(getVirtualMachineManager().isRemoteAttestationSupported())
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
    public void vmAttestationWithVendorPartitionWhenSupported() throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        assumeFeatureEnabled(VirtualMachineManager.FEATURE_REMOTE_ATTESTATION);
        assume().withMessage("Test needs Remote Attestation support")
                .that(getVirtualMachineManager().isRemoteAttestationSupported())
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
    public void vmAttestationWhenRemoteAttestationIsSupported() throws Exception {
        // pVM remote attestation is only supported on protected VMs.
        assumeProtectedVM();
        assumeFeatureEnabled(VirtualMachineManager.FEATURE_REMOTE_ATTESTATION);
        assume().withMessage("Test needs Remote Attestation support")
                .that(getVirtualMachineManager().isRemoteAttestationSupported())
                .isTrue();
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
            X509Utils.verifyAvfRelatedCerts(certs, challenge, TEST_APP_PACKAGE_NAME);
            X509Utils.verifySignature(
                    certs[0], VM_ATTESTATION_MESSAGE.getBytes(), signingResult.signature);
        }
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
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
    @CddTest(requirements = {"9.17/C-1-1"})
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
    @CddTest(requirements = {"9.17/C-1-1"})
    public void autoCloseVmDescriptor() throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        VirtualMachineDescriptor descriptor = vm.toDescriptor();

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
    @CddTest(requirements = {"9.17/C-1-1"})
    public void vmDescriptorClosedOnImport() throws Exception {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("test_vm", config);
        VirtualMachineDescriptor descriptor = vm.toDescriptor();

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
    @CddTest(requirements = {"9.17/C-1-1"})
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
        assertThrowsVmExceptionContaining(
                () -> vm.connectVsock(VirtualMachine.MIN_VSOCK_PORT), "not in running state");
        assertThrowsVmExceptionContaining(
                () -> vm.connectToVsockServer(VirtualMachine.MIN_VSOCK_PORT),
                "not in running state");

        vm.run();
        assertThat(vm.getStatus()).isEqualTo(STATUS_RUNNING);

        // These methods require a stopped VM
        assertThrowsVmExceptionContaining(() -> vm.run(), "not in stopped state");
        assertThrowsVmExceptionContaining(() -> vm.setConfig(config), "not in stopped state");
        assertThrowsVmExceptionContaining(() -> vm.toDescriptor(), "not in stopped state");
        assertThrowsVmExceptionContaining(
                () -> getVirtualMachineManager().delete("test_vm"), "not in stopped state");

        vm.stop();
        getVirtualMachineManager().delete("test_vm");
        assertThat(vm.getStatus()).isEqualTo(STATUS_DELETED);

        // None of these should work for a deleted VM
        assertThrowsVmExceptionContaining(
                () -> vm.connectVsock(VirtualMachine.MIN_VSOCK_PORT), "deleted");
        assertThrowsVmExceptionContaining(
                () -> vm.connectToVsockServer(VirtualMachine.MIN_VSOCK_PORT), "deleted");
        assertThrowsVmExceptionContaining(() -> vm.run(), "deleted");
        assertThrowsVmExceptionContaining(() -> vm.setConfig(config), "deleted");
        assertThrowsVmExceptionContaining(() -> vm.toDescriptor(), "deleted");
        // This is indistinguishable from the VM having never existed, so the message
        // is non-specific.
        assertThrowsVmException(() -> getVirtualMachineManager().delete("test_vm"));
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1"})
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
    @CddTest(requirements = {"9.17/C-1-1"})
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
    @CddTest(requirements = {"9.17/C-1-1"})
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

        assertThat(minimal.isCompatibleWith(maximal)).isFalse();
        assertThat(minimal.isCompatibleWith(minimal)).isTrue();
        assertThat(maximal.isCompatibleWith(maximal)).isTrue();
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1"})
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

        VirtualMachineConfig.Builder captureOutputOnNonDebuggable =
                newVmConfigBuilderWithPayloadBinary("binary.so")
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_NONE)
                        .setVmOutputCaptured(true);
        e = assertThrows(IllegalStateException.class, () -> captureOutputOnNonDebuggable.build());
        assertThat(e).hasMessageThat().contains("debug level must be FULL to capture output");

        VirtualMachineConfig.Builder captureInputOnNonDebuggable =
                newVmConfigBuilderWithPayloadBinary("binary.so")
                        .setDebugLevel(VirtualMachineConfig.DEBUG_LEVEL_NONE)
                        .setVmConsoleInputSupported(true);
        e = assertThrows(IllegalStateException.class, () -> captureInputOnNonDebuggable.build());
        assertThat(e).hasMessageThat().contains("debug level must be FULL to use console input");
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1"})
    public void compatibleConfigTests() {
        VirtualMachineConfig baseline = newBaselineBuilder().build();

        // A config must be compatible with itself
        assertConfigCompatible(baseline, newBaselineBuilder()).isTrue();

        // Changes that must always be compatible
        assertConfigCompatible(baseline, newBaselineBuilder().setMemoryBytes(99)).isTrue();
        assertConfigCompatible(
                        baseline, newBaselineBuilder().setCpuTopology(CPU_TOPOLOGY_MATCH_HOST))
                .isTrue();

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

        // Changes that are currently incompatible for ease of implementation, but this might change
        // in the future.
        assertConfigCompatible(baseline, newBaselineBuilder().setEncryptedStorageBytes(100_000))
                .isFalse();

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
    @CddTest(requirements = {"9.17/C-1-1"})
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
    @CddTest(requirements = {"9.17/C-1-1"})
    public void testAvfRequiresUpdatableApex() throws Exception {
        assertWithMessage("Devices that support AVF must also support updatable APEX")
                .that(SystemProperties.getBoolean("ro.apex.updatable", false))
                .isTrue();
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1"})
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
    @CddTest(requirements = {"9.17/C-1-1"})
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

    @Test
    @CddTest(requirements = {"9.17/C-1-1"})
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
    @CddTest(requirements = {"9.17/C-1-1"})
    public void differentManagersForDifferentContexts() throws Exception {
        final Context ceCtx = getContext().createCredentialProtectedStorageContext();
        final Context deCtx = getContext().createDeviceProtectedStorageContext();
        assertThat(ceCtx.getSystemService(VirtualMachineManager.class))
                .isNotSameInstanceAs(deCtx.getSystemService(VirtualMachineManager.class));
    }

    @Test
    @CddTest(
            requirements = {
                "9.17/C-1-1",
                "9.17/C-1-2",
                "9.17/C-1-4",
            })
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
    @CddTest(
            requirements = {
                "9.17/C-1-1",
            })
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
    @CddTest(
            requirements = {
                "9.17/C-1-1",
            })
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
    @CddTest(
            requirements = {
                "9.17/C-1-1",
            })
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
    @CddTest(requirements = {"9.17/C-1-1"})
    public void invalidVmNameIsRejected() {
        VirtualMachineManager vmm = getVirtualMachineManager();
        assertThrows(IllegalArgumentException.class, () -> vmm.get("../foo"));
        assertThrows(IllegalArgumentException.class, () -> vmm.get(".."));
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
    public void extraApkInVmConfig() throws Exception {
        assumeSupportedDevice();
        assumeFeatureEnabled(VirtualMachineManager.FEATURE_MULTI_TENANT);

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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-7"})
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-7"})
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-7"})
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-7"})
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-7"})
    public void bccIsSuperficiallyWellFormed() throws Exception {
        assumeSupportedDevice();

        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        VirtualMachineConfig normalConfig =
                newVmConfigBuilderWithPayloadConfig("assets/vm_config.json")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("bcc_vm", normalConfig);
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

        ByteArrayInputStream bais = new ByteArrayInputStream(bccBytes);
        List<DataItem> dataItems = new CborDecoder(bais).decode();
        assertThat(dataItems.size()).isEqualTo(1);
        assertThat(dataItems.get(0).getMajorType()).isEqualTo(MajorType.ARRAY);
        List<DataItem> rootArrayItems = ((Array) dataItems.get(0)).getDataItems();
        int diceChainSize = rootArrayItems.size();
        assertThat(diceChainSize).isAtLeast(2); // Root public key and one certificate
        if (mProtectedVm) {
            if (isFeatureEnabled(VirtualMachineManager.FEATURE_DICE_CHANGES)) {
                // We expect the root public key, at least one entry for the boot before pvmfw,
                // then pvmfw, vm_entry (Microdroid kernel) and Microdroid payload entries.
                // Before Android V we did not require that vendor code contain any DICE entries
                // preceding pvmfw, so the minimum is one less.
                int minDiceChainSize = getVendorApiLevel() > 202404 ? 5 : 4;
                assertThat(diceChainSize).isAtLeast(minDiceChainSize);
            } else {
                // pvmfw truncates the DICE chain it gets, so we expect exactly entries for
                // public key, vm_entry (Microdroid kernel) and Microdroid payload.
                assertThat(diceChainSize).isEqualTo(3);
            }
        }
    }

    @Test
    @VsrTest(requirements = {"VSR-7.1-001.004"})
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-1-2"})
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-7"})
    public void bootFailsWhenMicrodroidDataIsCompromised() throws Exception {
        // If Updatable VM is supported => No instance.img required
        assumeNoUpdatableVmSupport();
        assertThatBootFailsAfterCompromisingPartition(MICRODROID_PARTITION_UUID);
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-7"})
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
    public void bootFailsWhenApkPathIsInvalid() {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setApkPath("/does/not/exist")
                        .build();

        assertThrowsVmExceptionContaining(
                () -> tryBootVmWithConfig(config, "test_vm_invalid_apk_path"),
                "Failed to open APK");
    }

    @Test
    public void bootFailsWhenExtraApkPackageIsInvalid() {
        VirtualMachineConfig config =
                newVmConfigBuilderWithPayloadBinary("MicrodroidTestNativeLib.so")
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .addExtraApk("com.example.nosuch.package")
                        .build();
        assertThrowsVmExceptionContaining(
                () -> tryBootVmWithConfig(config, "test_vm_invalid_extra_apk_package"),
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
    @CddTest(requirements = {"9.17/C-1-1"})
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
    @CddTest(requirements = {"9.17/C-1-1"})
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
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-2-1"})
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
    @CddTest(requirements = {"9.17/C-1-1"})
    public void payloadIsNotRoot() throws Exception {
        assumeSupportedDevice();
        assumeFeatureEnabled(VirtualMachineManager.FEATURE_MULTI_TENANT);

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
    @CddTest(requirements = {"9.17/C-1-1"})
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
            assertThrowsVmExceptionContaining(
                    () -> vm.getConsoleOutput(), "Capturing vm outputs is turned off");
            assertThrowsVmExceptionContaining(
                    () -> vm.getLogOutput(), "Capturing vm outputs is turned off");
        } finally {
            vm.stop();
        }
    }

    @Test
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
            assertThrowsVmExceptionContaining(
                    () -> vm.getConsoleInput(), "VM console input is not supported");
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
                                "virtualizationmanager::aidl: (Console|Log).*executing main task",
                                "-t",
                                time)
                        .start();
        logcatProcess.waitFor();
        BufferedReader reader =
                new BufferedReader(new InputStreamReader(logcatProcess.getInputStream()));
        return !Strings.isNullOrEmpty(reader.readLine());
    }

    @Test
    public void outputIsRedirectedToLogcatIfNotCaptured() throws Exception {
        assumeSupportedDevice();

        assertThat(checkVmOutputIsRedirectedToLogcat(true)).isTrue();
    }

    private boolean setSystemProperties(String name, String value) {
        Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
        UiAutomation uiAutomation = instrumentation.getUiAutomation();
        String cmd = "setprop " + name + " " + (value.isEmpty() ? "\"\"" : value);
        return runInShellWithStderr(TAG, uiAutomation, cmd).trim().isEmpty();
    }

    @Test
    public void outputIsNotRedirectedToLogcatIfNotDebuggable() throws Exception {
        assumeSupportedDevice();

        // Disable debug policy to ensure no log output.
        String sysprop = "hypervisor.virtualizationmanager.debug_policy.path";
        String old = SystemProperties.get(sysprop);
        assumeTrue(
                "Can't disable debug policy. Perhapse user build?",
                setSystemProperties(sysprop, ""));

        try {
            assertThat(checkVmOutputIsRedirectedToLogcat(false)).isFalse();
        } finally {
            assertThat(setSystemProperties(sysprop, old)).isTrue();
        }
    }

    @Test
    public void testConsoleInputSupported() throws Exception {
        assumeSupportedDevice();
        assumeTrue("Not supported on GKI kernels", mGki == null);

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
    @CddTest(requirements = {"9.17/C-1-5"})
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
        if (isFeatureEnabled(VirtualMachineManager.FEATURE_MULTI_TENANT)) {
            expectedPermissions |= OsConstants.S_IRGRP | OsConstants.S_IXGRP;
        }
        assertThat(testResults.mFileMode & allPermissionsMask).isEqualTo(expectedPermissions);
    }

    // Taken from bionic/libc/kernel/uapi/linux/mount.h
    private static final int MS_RDONLY = 1;
    private static final int MS_NOEXEC = 8;
    private static final int MS_NOATIME = 1024;

    @Test
    @CddTest(requirements = {"9.17/C-1-5"})
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
    @CddTest(requirements = {"9.17/C-1-5"})
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
    @VsrTest(requirements = {"VSR-7.1-001.003"})
    public void kernelVersionRequirement() throws Exception {
        assumeVsrCompliant();
        int firstApiLevel = SystemProperties.getInt("ro.product.first_api_level", 0);
        assume().withMessage("Skip on devices launched before Android 14 (API level 34)")
                .that(firstApiLevel)
                .isAtLeast(34);

        String[] tokens = KERNEL_VERSION.split("\\.");
        int major = Integer.parseInt(tokens[0]);
        int minor = Integer.parseInt(tokens[1]);

        // Check kernel version >= 5.15
        assertTrue(major >= 5);
        if (major == 5) {
            assertTrue(minor >= 15);
        }
    }

    @Test
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

    private long getAvailableMemory() {
        ActivityManager am = getContext().getSystemService(ActivityManager.class);
        ActivityManager.MemoryInfo memoryInfo = new ActivityManager.MemoryInfo();
        am.getMemoryInfo(memoryInfo);
        return memoryInfo.availMem;
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

    private void assertThrowsVmException(ThrowingRunnable runnable) {
        assertThrows(VirtualMachineException.class, runnable);
    }

    private void assertThrowsVmExceptionContaining(
            ThrowingRunnable runnable, String expectedContents) {
        Exception e = assertThrows(VirtualMachineException.class, runnable);
        assertThat(e).hasMessageThat().contains(expectedContents);
    }

    private long minMemoryRequired() {
        assertThat(Build.SUPPORTED_ABIS).isNotEmpty();
        String primaryAbi = Build.SUPPORTED_ABIS[0];
        switch (primaryAbi) {
            case "x86_64":
                return MIN_MEM_X86_64;
            case "arm64-v8a":
            case "arm64-v8a-hwasan":
                return MIN_MEM_ARM64;
        }
        throw new AssertionError("Unsupported ABI: " + primaryAbi);
    }
}
