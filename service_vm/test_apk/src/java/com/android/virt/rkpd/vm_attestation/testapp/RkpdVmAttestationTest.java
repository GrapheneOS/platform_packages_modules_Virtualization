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

package com.android.virt.rkpd.vm_attestation.testapp;

import static android.system.virtualmachine.VirtualMachineConfig.DEBUG_LEVEL_FULL;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.Truth.assertWithMessage;
import static com.google.common.truth.TruthJUnit.assume;

import android.content.Context;
import android.hardware.security.keymint.IRemotelyProvisionedComponent;
import android.os.SystemProperties;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;

import androidx.work.ListenableWorker;
import androidx.work.testing.TestWorkerBuilder;

import com.android.microdroid.test.device.MicrodroidDeviceTestBase;
import com.android.rkpdapp.database.ProvisionedKeyDao;
import com.android.rkpdapp.database.RkpdDatabase;
import com.android.rkpdapp.interfaces.ServerInterface;
import com.android.rkpdapp.interfaces.ServiceManagerInterface;
import com.android.rkpdapp.interfaces.SystemInterface;
import com.android.rkpdapp.provisioner.PeriodicProvisioner;
import com.android.rkpdapp.testutil.SystemInterfaceSelector;
import com.android.rkpdapp.utils.Settings;
import com.android.rkpdapp.utils.X509Utils;
import com.android.virt.vm_attestation.testservice.IAttestationService;
import com.android.virt.vm_attestation.testservice.IAttestationService.SigningResult;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;

import java.security.Signature;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Executors;

/**
 * End-to-end test for the pVM remote attestation.
 *
 * <p>The test checks the two major steps of the pVM remote attestation:
 *
 * <p>1. Key provisioning: The test provisions AVF keys from the RKP server and verifies that the
 * keys are for AVF.
 *
 * <p>2. VM attestation: The test creates a VM with a payload binary that requests to attest the VM,
 * and then signs a message with the attestation key.
 *
 * <p>To run this test, you need to:
 *
 * <p>- Have an arm64 device supporting protected VMs.
 *
 * <p>- Have a stable network connection on the device.
 *
 * <p>- Have the RKP server hostname configured in the device. If not, you can set it using: $ adb
 * shell setprop remote_provisioning.hostname remoteprovisioning.googleapis.com
 */
@RunWith(Parameterized.class)
public class RkpdVmAttestationTest extends MicrodroidDeviceTestBase {
    private static final String TAG = "RkpdVmAttestationTest";
    private static final String AVF_ATTESTATION_EXTENSION_OID = "1.3.6.1.4.1.11129.2.1.29.1";
    private static final String SERVICE_NAME = IRemotelyProvisionedComponent.DESCRIPTOR + "/avf";
    private static final String VM_PAYLOAD_PATH = "libvm_attestation_test_payload.so";
    private static final String MESSAGE = "Hello RKP from AVF!";
    private static final String TEST_APP_PACKAGE_NAME =
            "com.android.virt.rkpd.vm_attestation.testapp";

    private ProvisionedKeyDao mKeyDao;
    private PeriodicProvisioner mProvisioner;

    @Parameterized.Parameter(0)
    public String mGki;

    @Parameterized.Parameters(name = "gki={0}")
    public static Collection<Object[]> params() {
        List<Object[]> ret = new ArrayList<>();
        ret.add(new Object[] {null /* use microdroid kernel */});
        for (String gki : SUPPORTED_GKI_VERSIONS) {
            ret.add(new Object[] {gki});
        }
        return ret;
    }

    @Before
    public void setUp() throws Exception {
        assume().withMessage("The RKP server hostname is not configured -- assume RKP disabled.")
                .that(SystemProperties.get("remote_provisioning.hostname"))
                .isNotEmpty();
        assume().withMessage("RKP Integration tests rely on network availability.")
                .that(ServerInterface.isNetworkConnected(getContext()))
                .isTrue();
        // TODO(b/329652894): Assume that pVM remote attestation feature is supported.

        prepareTestSetup(true /* protectedVm */, mGki);

        Settings.clearPreferences(getContext());
        mKeyDao = RkpdDatabase.getDatabase(getContext()).provisionedKeyDao();
        mKeyDao.deleteAllKeys();

        mProvisioner =
                TestWorkerBuilder.from(
                                getContext(),
                                PeriodicProvisioner.class,
                                Executors.newSingleThreadExecutor())
                        .build();

        SystemInterface systemInterface =
                SystemInterfaceSelector.getSystemInterfaceForServiceName(SERVICE_NAME);
        ServiceManagerInterface.setInstances(new SystemInterface[] {systemInterface});

        setMaxPerformanceTaskProfile();
    }

    @After
    public void tearDown() throws Exception {
        ServiceManagerInterface.setInstances(null);
        if (mKeyDao != null) {
            mKeyDao.deleteAllKeys();
        }
        Settings.clearPreferences(getContext());
    }

    @Test
    public void usingProvisionedKeyForVmAttestationSucceeds() throws Exception {
        // Provision keys.
        assertThat(mProvisioner.doWork()).isEqualTo(ListenableWorker.Result.success());
        assertThat(mKeyDao.getTotalUnassignedKeysForIrpc(SERVICE_NAME)).isGreaterThan(0);

        // Arrange.
        Context ctx = getContext();
        Context otherAppCtx = ctx.createPackageContext(TEST_APP_PACKAGE_NAME, 0);
        VirtualMachineConfig config =
                new VirtualMachineConfig.Builder(otherAppCtx)
                        .setProtectedVm(true)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setPayloadBinaryName(VM_PAYLOAD_PATH)
                        .setVmOutputCaptured(true)
                        .build();
        VirtualMachine vm = forceCreateNewVirtualMachine("attestation_with_rkpd_client", config);
        byte[] challenge = new byte[32];
        Arrays.fill(challenge, (byte) 0xab);

        // Act.
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Boolean> payloadReady = new CompletableFuture<>();
        CompletableFuture<SigningResult> signingResultFuture = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        payloadReady.complete(true);
                        try {
                            IAttestationService service =
                                    IAttestationService.Stub.asInterface(
                                            vm.connectToVsockServer(IAttestationService.PORT));
                            signingResultFuture.complete(
                                    service.signWithAttestationKey(challenge, MESSAGE.getBytes()));
                        } catch (Exception e) {
                            exception.complete(e);
                        } finally {
                            forceStop(vm);
                        }
                    }
                };
        listener.runToFinish(TAG, vm);

        // Assert.
        assertThat(payloadReady.getNow(false)).isTrue();
        assertThat(exception.getNow(null)).isNull();
        SigningResult signingResult = signingResultFuture.getNow(null);
        assertThat(signingResult).isNotNull();

        // Parsing the certificate chain successfully indicates that the certificate
        // chain is valid, that each certificate is signed by the next one and the last
        // one is self-signed.
        X509Certificate[] certs = X509Utils.formatX509Certs(signingResult.certificateChain);
        assertThat(certs.length).isGreaterThan(2);
        assertWithMessage("The first certificate should be generated in the RKP VM")
                .that(certs[0].getSubjectX500Principal().getName())
                .isEqualTo("CN=Android Protected Virtual Machine Key");
        checkAvfAttestationExtension(certs[0], challenge);
        assertWithMessage("The second certificate should contain AVF in the subject")
                .that(certs[1].getSubjectX500Principal().getName())
                .contains("O=AVF");

        // Verify the signature using the public key from the leaf certificate generated
        // in the RKP VM.
        Signature sig = Signature.getInstance("SHA256withECDSA");
        sig.initVerify(certs[0].getPublicKey());
        sig.update(MESSAGE.getBytes());
        assertThat(sig.verify(signingResult.signature)).isTrue();
    }

    private void checkAvfAttestationExtension(X509Certificate cert, byte[] challenge) {
        byte[] extension = cert.getExtensionValue(AVF_ATTESTATION_EXTENSION_OID);
        assertThat(extension).isNotNull();
        // TODO(b/325610326): Use bouncycastle to parse the extension and check other fields.
        assertWithMessage("The extension should contain the challenge")
                .that(containsSubarray(extension, challenge))
                .isTrue();
    }

    private boolean containsSubarray(byte[] array, byte[] subarray) {
        for (int i = 0; i < array.length - subarray.length + 1; i++) {
            boolean found = true;
            for (int j = 0; j < subarray.length; j++) {
                if (array[i + j] != subarray[j]) {
                    found = false;
                    break;
                }
            }
            if (found) {
                return true;
            }
        }
        return false;
    }
}
