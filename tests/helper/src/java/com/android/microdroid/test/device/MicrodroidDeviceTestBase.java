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
package com.android.microdroid.test.device;

import static android.content.pm.PackageManager.FEATURE_AUTOMOTIVE;
import static android.content.pm.PackageManager.FEATURE_LEANBACK;
import static android.content.pm.PackageManager.FEATURE_VIRTUALIZATION_FRAMEWORK;
import static android.content.pm.PackageManager.FEATURE_WATCH;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.TruthJUnit.assume;

import static org.junit.Assume.assumeFalse;
import static org.junit.Assume.assumeTrue;

import android.app.Instrumentation;
import android.app.UiAutomation;
import android.content.Context;
import android.os.ParcelFileDescriptor;
import android.os.SystemProperties;
import android.system.Os;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineCallback;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineException;
import android.system.virtualmachine.VirtualMachineManager;
import android.util.Log;

import androidx.annotation.CallSuper;
import androidx.test.core.app.ApplicationProvider;
import androidx.test.platform.app.InstrumentationRegistry;

import com.android.microdroid.test.common.DeviceProperties;
import com.android.microdroid.test.common.MetricsProcessor;
import com.android.microdroid.testservice.ITestService;
import com.android.virt.vm_attestation.testservice.IAttestationService;
import com.android.virt.vm_attestation.testservice.IAttestationService.SigningResult;

import java.io.BufferedReader;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.OptionalLong;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

public abstract class MicrodroidDeviceTestBase {
    private static final String TAG = "MicrodroidDeviceTestBase";
    private final String MAX_PERFORMANCE_TASK_PROFILE = "CPUSET_SP_TOP_APP";

    protected static final String KERNEL_VERSION = SystemProperties.get("ro.kernel.version");
    protected static final Set<String> SUPPORTED_GKI_VERSIONS =
            Collections.unmodifiableSet(new HashSet(Arrays.asList("android15-6.6")));

    public static boolean isCuttlefish() {
        return getDeviceProperties().isCuttlefish();
    }

    private static boolean isCuttlefishArm64() {
        return getDeviceProperties().isCuttlefishArm64();
    }

    public static boolean isGoldfish() {
        return getDeviceProperties().isGoldfish();
    }

    private static boolean isGoldfishArm64() {
        return getDeviceProperties().isGoldfishArm64();
    }

    public static boolean isHwasan() {
        return getDeviceProperties().isHwasan();
    }

    public static boolean isUserBuild() {
        return getDeviceProperties().isUserBuild();
    }

    public static String getMetricPrefix() {
        return MetricsProcessor.getMetricPrefix(getDeviceProperties().getMetricsTag());
    }

    private static DeviceProperties getDeviceProperties() {
        return DeviceProperties.create(SystemProperties::get);
    }

    protected final void grantPermission(String permission) {
        Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
        UiAutomation uiAutomation = instrumentation.getUiAutomation();
        uiAutomation.grantRuntimePermission(
                instrumentation.getContext().getPackageName(), permission);
    }

    protected final void revokePermission(String permission) {
        Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
        UiAutomation uiAutomation = instrumentation.getUiAutomation();
        uiAutomation.revokeRuntimePermission(
                instrumentation.getContext().getPackageName(), permission);
    }

    protected final void setMaxPerformanceTaskProfile() throws IOException {
        Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
        UiAutomation uiAutomation = instrumentation.getUiAutomation();
        String cmd = "settaskprofile " + Os.gettid() + " " + MAX_PERFORMANCE_TASK_PROFILE;
        String out = runInShell(TAG, uiAutomation, cmd).trim();
        String expect = "Profile " + MAX_PERFORMANCE_TASK_PROFILE + " is applied successfully!";
        if (!expect.equals(out)) {
            throw new IOException("Could not apply max performance task profile: " + out);
        }
    }

    private final Context mCtx = ApplicationProvider.getApplicationContext();
    private boolean mProtectedVm;
    private String mGki;

    protected Context getContext() {
        return mCtx;
    }

    public VirtualMachineManager getVirtualMachineManager() {
        return mCtx.getSystemService(VirtualMachineManager.class);
    }

    public VirtualMachineConfig.Builder newVmConfigBuilderWithPayloadConfig(String configPath) {
        return new VirtualMachineConfig.Builder(mCtx)
                .setProtectedVm(mProtectedVm)
                .setOs(os())
                .setPayloadConfigPath(configPath);
    }

    public VirtualMachineConfig.Builder newVmConfigBuilderWithPayloadBinary(String binaryPath) {
        return new VirtualMachineConfig.Builder(mCtx)
                .setProtectedVm(mProtectedVm)
                .setOs(os())
                .setPayloadBinaryName(binaryPath);
    }

    protected final boolean isProtectedVm() {
        return mProtectedVm;
    }

    protected final String os() {
        return mGki != null ? "microdroid_gki-" + mGki : "microdroid";
    }

    /**
     * Creates a new virtual machine, potentially removing an existing virtual machine with given
     * name.
     */
    public VirtualMachine forceCreateNewVirtualMachine(String name, VirtualMachineConfig config)
            throws VirtualMachineException {
        final VirtualMachineManager vmm = getVirtualMachineManager();
        deleteVirtualMachineIfExists(name);
        return vmm.create(name, config);
    }

    protected void deleteVirtualMachineIfExists(String name) throws VirtualMachineException {
        VirtualMachineManager vmm = getVirtualMachineManager();
        boolean deleteExisting;
        try {
            deleteExisting = vmm.get(name) != null;
        } catch (VirtualMachineException e) {
            // VM exists, i.e. there are some files for it, but they could not be successfully
            // loaded.
            deleteExisting = true;
        }
        if (deleteExisting) {
            vmm.delete(name);
        }
    }

    public void prepareTestSetup(boolean protectedVm, String gki) {
        assumeFeatureVirtualizationFramework();

        mProtectedVm = protectedVm;
        mGki = gki;

        int capabilities = getVirtualMachineManager().getCapabilities();
        if (protectedVm) {
            assume().withMessage("Skip where protected VMs aren't supported")
                    .that(capabilities & VirtualMachineManager.CAPABILITY_PROTECTED_VM)
                    .isNotEqualTo(0);
            assume().withMessage("Testing protected VMs on GSI isn't supported. b/272443823")
                    .that(isGsi())
                    .isFalse();
        } else {
            assume().withMessage("Skip where VMs aren't supported")
                    .that(capabilities & VirtualMachineManager.CAPABILITY_NON_PROTECTED_VM)
                    .isNotEqualTo(0);
        }

        try {
            assume().withMessage("Skip where requested OS \"" + os() + "\" isn't supported")
                    .that(os())
                    .isIn(getVirtualMachineManager().getSupportedOSList());
        } catch (VirtualMachineException e) {
            Log.e(TAG, "Error getting supported OS list", e);
            throw new RuntimeException("Failed to get supported OS list.", e);
        }
    }

    protected void assumeFeatureVirtualizationFramework() {
        assume().withMessage("Device doesn't support AVF")
                .that(mCtx.getPackageManager().hasSystemFeature(FEATURE_VIRTUALIZATION_FRAMEWORK))
                .isTrue();
        int vendorApiLevel = getVendorApiLevel();
        boolean isGsi = isGsi();
        Log.i(TAG, "isGsi = " + isGsi + ", vendor api level = " + vendorApiLevel);
        assume().withMessage("GSI with vendor API level < 202404 may not support AVF")
                .that(isGsi && vendorApiLevel < 202404)
                .isFalse();
    }

    protected void assumeVsrCompliant() {
        boolean featureCheck =
                mCtx.getPackageManager().hasSystemFeature(FEATURE_WATCH)
                        || mCtx.getPackageManager().hasSystemFeature(FEATURE_AUTOMOTIVE)
                        || mCtx.getPackageManager().hasSystemFeature(FEATURE_LEANBACK);
        assume().withMessage("This device is not VSR compliant").that(featureCheck).isFalse();
    }

    protected boolean isGsi() {
        return new File("/system/system_ext/etc/init/init.gsi.rc").exists();
    }

    protected static int getVendorApiLevel() {
        return SystemProperties.getInt("ro.board.api_level", 0);
    }

    protected void assumeSupportedDevice() {
        assume().withMessage("Skip on 5.4 kernel. b/218303240")
                .that(KERNEL_VERSION)
                .isNotEqualTo("5.4");

        // Cuttlefish/Goldfish on Arm 64 doesn't and cannot support any form of virtualization,
        // so there's no point running any of these tests.
        assume().withMessage(
                        "Virtualization not supported on Arm64 Cuttlefish/Goldfish."
                                + " b/341889915")
                .that(isCuttlefishArm64() || isGoldfishArm64())
                .isFalse();
    }

    protected void assumeNoUpdatableVmSupport() throws VirtualMachineException {
        assume().withMessage("Secretkeeper not supported")
                .that(getVirtualMachineManager().isUpdatableVmSupported())
                .isFalse();
    }

    public abstract static class VmEventListener implements VirtualMachineCallback {
        private ExecutorService mExecutorService = Executors.newSingleThreadExecutor();
        private OptionalLong mVcpuStartedNanoTime = OptionalLong.empty();
        private OptionalLong mKernelStartedNanoTime = OptionalLong.empty();
        private OptionalLong mInitStartedNanoTime = OptionalLong.empty();
        private OptionalLong mPayloadStartedNanoTime = OptionalLong.empty();
        private StringBuilder mConsoleOutput = new StringBuilder();
        private StringBuilder mLogOutput = new StringBuilder();
        private boolean mProcessedBootTimeMetrics = false;

        private synchronized void processBootTimeMetrics(String log) {
            if (!mVcpuStartedNanoTime.isPresent()) {
                mVcpuStartedNanoTime = OptionalLong.of(System.nanoTime());
            }
            if (log.contains("Starting payload...") && !mKernelStartedNanoTime.isPresent()) {
                mKernelStartedNanoTime = OptionalLong.of(System.nanoTime());
            }
            if (log.contains("Run /init as init process") && !mInitStartedNanoTime.isPresent()) {
                mInitStartedNanoTime = OptionalLong.of(System.nanoTime());
            }
            if (log.contains("microdroid_manager")
                    && log.contains("executing main task")
                    && !mPayloadStartedNanoTime.isPresent()) {
                mPayloadStartedNanoTime = OptionalLong.of(System.nanoTime());
            }
        }

        private void logVmOutputAndMonitorBootTimeMetrics(
                String tag, InputStream vmOutputStream, String name, StringBuilder result) {
            mProcessedBootTimeMetrics = true;
            new Thread(
                            () -> {
                                try {
                                    BufferedReader reader =
                                            new BufferedReader(
                                                    new InputStreamReader(vmOutputStream));
                                    String line;
                                    while ((line = reader.readLine()) != null
                                            && !Thread.interrupted()) {
                                        processBootTimeMetrics(line);
                                        Log.i(tag, name + ": " + line);
                                        result.append(line + "\n");
                                    }
                                } catch (Exception e) {
                                    Log.w(tag, name, e);
                                }
                            })
                    .start();
        }

        public void runToFinish(String logTag, VirtualMachine vm)
                throws VirtualMachineException, InterruptedException {
            vm.setCallback(mExecutorService, this);
            vm.run();
            if (vm.getConfig().isVmOutputCaptured()) {
                logVmOutputAndMonitorBootTimeMetrics(
                        logTag, vm.getConsoleOutput(), "Console", mConsoleOutput);
                logVmOutputAndMonitorBootTimeMetrics(logTag, vm.getLogOutput(), "Log", mLogOutput);
            }
            mExecutorService.awaitTermination(300, TimeUnit.SECONDS);
        }

        public OptionalLong getVcpuStartedNanoTime() {
            return mVcpuStartedNanoTime;
        }

        public OptionalLong getKernelStartedNanoTime() {
            return mKernelStartedNanoTime;
        }

        public OptionalLong getInitStartedNanoTime() {
            return mInitStartedNanoTime;
        }

        public OptionalLong getPayloadStartedNanoTime() {
            return mPayloadStartedNanoTime;
        }

        public String getConsoleOutput() {
            return mConsoleOutput.toString();
        }

        public String getLogOutput() {
            return mLogOutput.toString();
        }

        public boolean hasProcessedBootTimeMetrics() {
            return mProcessedBootTimeMetrics;
        }

        protected void forceStop(VirtualMachine vm) {
            try {
                vm.stop();
            } catch (VirtualMachineException e) {
                throw new RuntimeException(e);
            }
        }

        @Override
        public void onPayloadStarted(VirtualMachine vm) {}

        @Override
        public void onPayloadReady(VirtualMachine vm) {}

        @Override
        public void onPayloadFinished(VirtualMachine vm, int exitCode) {}

        @Override
        public void onError(VirtualMachine vm, int errorCode, String message) {}

        @Override
        @CallSuper
        public void onStopped(VirtualMachine vm, int reason) {
            vm.clearCallback();
            mExecutorService.shutdown();
        }
    }

    public enum BootTimeMetric {
        TOTAL,
        VM_START,
        BOOTLOADER,
        KERNEL,
        USERSPACE,
    }

    public static class BootResult {
        public final boolean payloadStarted;
        public final int deathReason;
        public final long apiCallNanoTime;
        public final long endToEndNanoTime;

        public final boolean processedBootTimeMetrics;
        public final OptionalLong vcpuStartedNanoTime;
        public final OptionalLong kernelStartedNanoTime;
        public final OptionalLong initStartedNanoTime;
        public final OptionalLong payloadStartedNanoTime;

        public final String consoleOutput;
        public final String logOutput;

        BootResult(
                boolean payloadStarted,
                int deathReason,
                long apiCallNanoTime,
                long endToEndNanoTime,
                boolean processedBootTimeMetrics,
                OptionalLong vcpuStartedNanoTime,
                OptionalLong kernelStartedNanoTime,
                OptionalLong initStartedNanoTime,
                OptionalLong payloadStartedNanoTime,
                String consoleOutput,
                String logOutput) {
            this.apiCallNanoTime = apiCallNanoTime;
            this.payloadStarted = payloadStarted;
            this.deathReason = deathReason;
            this.endToEndNanoTime = endToEndNanoTime;
            this.processedBootTimeMetrics = processedBootTimeMetrics;
            this.vcpuStartedNanoTime = vcpuStartedNanoTime;
            this.kernelStartedNanoTime = kernelStartedNanoTime;
            this.initStartedNanoTime = initStartedNanoTime;
            this.payloadStartedNanoTime = payloadStartedNanoTime;
            this.consoleOutput = consoleOutput;
            this.logOutput = logOutput;
        }

        private long getVcpuStartedNanoTime() {
            return vcpuStartedNanoTime.getAsLong();
        }

        private long getKernelStartedNanoTime() {
            // pvmfw emits log at the end which is used to estimate the kernelStart time.
            // In case of no pvmfw run(non-protected mode), use vCPU started time instead.
            return kernelStartedNanoTime.orElse(vcpuStartedNanoTime.getAsLong());
        }

        private long getInitStartedNanoTime() {
            return initStartedNanoTime.getAsLong();
        }

        private long getPayloadStartedNanoTime() {
            return payloadStartedNanoTime.getAsLong();
        }

        public long getVMStartingElapsedNanoTime() {
            return getVcpuStartedNanoTime() - apiCallNanoTime;
        }

        public long getBootloaderElapsedNanoTime() {
            return getKernelStartedNanoTime() - getVcpuStartedNanoTime();
        }

        public long getKernelElapsedNanoTime() {
            return getInitStartedNanoTime() - getKernelStartedNanoTime();
        }

        public long getUserspaceElapsedNanoTime() {
            return getPayloadStartedNanoTime() - getInitStartedNanoTime();
        }

        public OptionalLong getBootTimeMetricNanoTime(BootTimeMetric metric) {
            if (metric == BootTimeMetric.TOTAL) {
                return OptionalLong.of(endToEndNanoTime);
            }

            if (processedBootTimeMetrics) {
                switch (metric) {
                    case VM_START:
                        return OptionalLong.of(getVMStartingElapsedNanoTime());
                    case BOOTLOADER:
                        return OptionalLong.of(getBootloaderElapsedNanoTime());
                    case KERNEL:
                        return OptionalLong.of(getKernelElapsedNanoTime());
                    case USERSPACE:
                        return OptionalLong.of(getUserspaceElapsedNanoTime());
                }
            }

            return OptionalLong.empty();
        }
    }

    public BootResult tryBootVm(String logTag, String vmName)
            throws VirtualMachineException, InterruptedException {
        VirtualMachine vm = getVirtualMachineManager().get(vmName);
        final CompletableFuture<Boolean> payloadStarted = new CompletableFuture<>();
        final CompletableFuture<Integer> deathReason = new CompletableFuture<>();
        final CompletableFuture<Long> endTime = new CompletableFuture<>();
        VmEventListener listener =
                new VmEventListener() {
                    @Override
                    public void onPayloadStarted(VirtualMachine vm) {
                        endTime.complete(System.nanoTime());
                        payloadStarted.complete(true);
                        forceStop(vm);
                    }

                    @Override
                    public void onStopped(VirtualMachine vm, int reason) {
                        deathReason.complete(reason);
                        super.onStopped(vm, reason);
                    }
                };
        long apiCallNanoTime = System.nanoTime();
        listener.runToFinish(logTag, vm);
        return new BootResult(
                payloadStarted.getNow(false),
                deathReason.getNow(VmEventListener.STOP_REASON_INFRASTRUCTURE_ERROR),
                apiCallNanoTime,
                endTime.getNow(apiCallNanoTime) - apiCallNanoTime,
                listener.hasProcessedBootTimeMetrics(),
                listener.getVcpuStartedNanoTime(),
                listener.getKernelStartedNanoTime(),
                listener.getInitStartedNanoTime(),
                listener.getPayloadStartedNanoTime(),
                listener.getConsoleOutput(),
                listener.getLogOutput());
    }

    /** Execute a command. Returns stdout. */
    protected String runInShell(String tag, UiAutomation uiAutomation, String command) {
        try (InputStream is =
                        new ParcelFileDescriptor.AutoCloseInputStream(
                                uiAutomation.executeShellCommand(command));
                ByteArrayOutputStream out = new ByteArrayOutputStream()) {
            is.transferTo(out);
            String stdout = out.toString("UTF-8");
            Log.i(tag, "Got stdout : " + stdout);
            return stdout;
        } catch (IOException e) {
            Log.e(tag, "Error executing: " + command, e);
            throw new RuntimeException("Failed to run the command.", e);
        }
    }

    /** Execute a command. Returns the concatenation of stdout and stderr. */
    protected String runInShellWithStderr(String tag, UiAutomation uiAutomation, String command) {
        ParcelFileDescriptor[] files = uiAutomation.executeShellCommandRwe(command);
        try (InputStream stdout = new ParcelFileDescriptor.AutoCloseInputStream(files[0]);
                InputStream stderr = new ParcelFileDescriptor.AutoCloseInputStream(files[2]);
                ByteArrayOutputStream out = new ByteArrayOutputStream()) {
            files[1].close(); // The command's stdin
            stdout.transferTo(out);
            stderr.transferTo(out);
            String output = out.toString("UTF-8");
            Log.i(tag, "Got output : " + stdout);
            return output;
        } catch (IOException e) {
            Log.e(tag, "Error executing: " + command, e);
            throw new RuntimeException("Failed to run the command.", e);
        }
    }

    protected static class TestResults {
        public Exception mException;
        public Integer mAddInteger;
        public String mAppRunProp;
        public String mSublibRunProp;
        public String mExtraApkTestProp;
        public String mApkContentsPath;
        public String mEncryptedStoragePath;
        public String[] mEffectiveCapabilities;
        public int mUid;
        public String mFileContent;
        public byte[] mBcc;
        public long[] mTimings;
        public int mFileMode;
        public int mMountFlags;
        public String mConsoleInput;
        public byte[] mInstanceSecret;

        public void assertNoException() {
            if (mException != null) {
                // Rethrow, wrapped in a new exception, so we get stack traces of the original
                // failure as well as the body of the test.
                throw new RuntimeException(mException);
            }
        }
    }

    protected SigningResult runVmAttestationService(
            String logTag, VirtualMachine vm, byte[] challenge, byte[] messageToSign)
            throws Exception {

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
                                    service.signWithAttestationKey(challenge, messageToSign));
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
        SigningResult signingResult = signingResultFuture.getNow(null);
        assertThat(signingResult).isNotNull();
        return signingResult;
    }

    protected TestResults runVmTestService(
            String logTag, VirtualMachine vm, RunTestsAgainstTestService testsToRun)
            throws Exception {
        CompletableFuture<Boolean> payloadStarted = new CompletableFuture<>();
        CompletableFuture<Boolean> payloadReady = new CompletableFuture<>();
        CompletableFuture<Boolean> payloadFinished = new CompletableFuture<>();
        TestResults testResults = new TestResults();
        VmEventListener listener =
                new VmEventListener() {
                    ITestService mTestService = null;

                    private void initializeTestService(VirtualMachine vm) {
                        try {
                            mTestService =
                                    ITestService.Stub.asInterface(
                                            vm.connectToVsockServer(ITestService.PORT));
                            // Make sure linkToDeath works, and include it in the log in case it's
                            // helpful.
                            mTestService
                                    .asBinder()
                                    .linkToDeath(
                                            () -> Log.i(logTag, "ITestService binder died"), 0);
                        } catch (Exception e) {
                            testResults.mException = e;
                        }
                    }

                    private void testVMService(VirtualMachine vm) {
                        try {
                            if (mTestService == null) initializeTestService(vm);
                            testsToRun.runTests(mTestService, testResults);
                        } catch (Exception e) {
                            testResults.mException = e;
                        }
                    }

                    private void quitVMService() {
                        try {
                            mTestService.quit();
                        } catch (Exception e) {
                            testResults.mException = e;
                        }
                    }

                    @Override
                    public void onPayloadReady(VirtualMachine vm) {
                        Log.i(logTag, "onPayloadReady");
                        payloadReady.complete(true);
                        testVMService(vm);
                        quitVMService();
                    }

                    @Override
                    public void onPayloadStarted(VirtualMachine vm) {
                        Log.i(logTag, "onPayloadStarted");
                        payloadStarted.complete(true);
                    }

                    @Override
                    public void onPayloadFinished(VirtualMachine vm, int exitCode) {
                        Log.i(logTag, "onPayloadFinished: " + exitCode);
                        payloadFinished.complete(true);
                        forceStop(vm);
                    }
                };

        listener.runToFinish(logTag, vm);
        assertThat(payloadStarted.getNow(false)).isTrue();
        assertThat(payloadReady.getNow(false)).isTrue();
        assertThat(payloadFinished.getNow(false)).isTrue();
        return testResults;
    }

    @FunctionalInterface
    protected interface RunTestsAgainstTestService {
        void runTests(ITestService testService, TestResults testResults) throws Exception;
    }

    protected void assumeFeatureEnabled(String featureName) throws Exception {
        assumeTrue(featureName + " not enabled", isFeatureEnabled(featureName));
    }

    protected boolean isFeatureEnabled(String featureName) throws Exception {
        return getVirtualMachineManager().isFeatureEnabled(featureName);
    }

    protected void assumeProtectedVM() {
        assumeTrue("Skip on non-protected VM", mProtectedVm);
    }

    protected void assumeNonProtectedVM() {
        assumeFalse("Skip on protected VM", mProtectedVm);
    }
}
