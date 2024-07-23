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

package com.android.virt.vm_attestation.testapp;

import static android.system.virtualmachine.VirtualMachineConfig.DEBUG_LEVEL_FULL;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.TruthJUnit.assume;

import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineManager;

import com.android.microdroid.test.device.MicrodroidDeviceTestBase;
import com.android.virt.vm_attestation.testservice.IAttestationService;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;

import java.io.IOException;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.concurrent.CompletableFuture;

@RunWith(Parameterized.class)
public class VmAttestationTests extends MicrodroidDeviceTestBase {
    private static final String TAG = "VmAttestationTest";
    private static final String VM_PAYLOAD_PATH = "libvm_attestation_test_payload.so";

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
    public void setup() throws IOException {
        grantPermission(VirtualMachine.MANAGE_VIRTUAL_MACHINE_PERMISSION);
        grantPermission(VirtualMachine.USE_CUSTOM_VIRTUAL_MACHINE_PERMISSION);
        prepareTestSetup(true /* protectedVm */, mGki);
        setMaxPerformanceTaskProfile();
    }

    @Test
    public void requestingAttestationSucceeds() throws Exception {
        assume().withMessage("Remote attestation is not supported on CF.")
                .that(isCuttlefish())
                .isFalse();
        assumeFeatureEnabled(VirtualMachineManager.FEATURE_REMOTE_ATTESTATION);
        assume().withMessage("Test needs Remote Attestation support")
                .that(getVirtualMachineManager().isRemoteAttestationSupported())
                .isTrue();

        VirtualMachineConfig.Builder builder =
                newVmConfigBuilderWithPayloadBinary(VM_PAYLOAD_PATH)
                        .setDebugLevel(DEBUG_LEVEL_FULL)
                        .setVmOutputCaptured(true);
        VirtualMachineConfig config = builder.build();
        VirtualMachine vm = forceCreateNewVirtualMachine("attestation_client", config);

        vm.enableTestAttestation();
        CompletableFuture<Exception> exception = new CompletableFuture<>();
        CompletableFuture<Boolean> payloadReady = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        payloadReady.complete(true);
                        try {
                            IAttestationService service =
                                    IAttestationService.Stub.asInterface(
                                            vm.connectToVsockServer(IAttestationService.PORT));
                            android.os.Trace.beginSection("runningVmRequestsAttestation");
                            service.requestAttestationForTesting();
                            android.os.Trace.endSection();
                            service.validateAttestationResult();
                        } catch (Exception e) {
                            exception.complete(e);
                        } finally {
                            forceStop(vm);
                        }
                    }
                };

        listener.runToFinish(TAG, vm);
        assertThat(payloadReady.getNow(false)).isTrue();
        assertThat(exception.getNow(null)).isNull();
    }
}
