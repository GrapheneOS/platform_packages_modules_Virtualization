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

import static com.android.microdroid.test.host.CommandResultSubject.command_results;
import static com.android.tradefed.device.TestDevice.MicrodroidBuilder;
import static com.android.tradefed.testtype.DeviceJUnit4ClassRunner.TestLogData;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.Truth.assertWithMessage;

import static org.hamcrest.CoreMatchers.containsString;
import static org.junit.Assert.assertThat;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;
import static org.junit.Assume.assumeFalse;
import static org.junit.Assume.assumeTrue;

import static java.util.stream.Collectors.toList;

import android.cts.host.utils.DeviceJUnit4ClassRunnerWithParameters;
import android.cts.host.utils.DeviceJUnit4Parameterized;
import android.cts.statsdatom.lib.ConfigUtils;
import android.cts.statsdatom.lib.ReportUtils;

import com.android.compatibility.common.util.CddTest;
import com.android.microdroid.test.common.ProcessUtil;
import com.android.microdroid.test.host.CommandRunner;
import com.android.microdroid.test.host.MicrodroidHostTestCaseBase;
import com.android.os.AtomsProto;
import com.android.os.StatsLog;
import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.device.DeviceRuntimeException;
import com.android.tradefed.device.ITestDevice;
import com.android.tradefed.device.TestDevice;
import com.android.tradefed.testtype.DeviceJUnit4ClassRunner.TestMetrics;
import com.android.tradefed.util.CommandResult;
import com.android.tradefed.util.CommandStatus;
import com.android.tradefed.util.FileUtil;
import com.android.tradefed.util.RunUtil;
import com.android.tradefed.util.xml.AbstractXmlParser;
import com.android.virt.PayloadMetadata;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Before;
import org.junit.Ignore;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TestName;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;
import org.junit.runners.Parameterized.UseParametersRunnerFactory;
import org.xml.sax.Attributes;
import org.xml.sax.helpers.DefaultHandler;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.PipedInputStream;
import java.io.PipedOutputStream;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.Callable;
import java.util.concurrent.TimeUnit;
import java.util.function.Function;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Collectors;

@RunWith(DeviceJUnit4Parameterized.class)
@UseParametersRunnerFactory(DeviceJUnit4ClassRunnerWithParameters.RunnerFactory.class)
public class MicrodroidHostTests extends MicrodroidHostTestCaseBase {
    private static final String APK_NAME = "MicrodroidTestApp.apk";
    private static final String APK_UPDATED_NAME = "MicrodroidTestAppUpdated.apk";
    private static final String PACKAGE_NAME = "com.android.microdroid.test";
    private static final String SHELL_PACKAGE_NAME = "com.android.shell";
    private static final String VIRT_APEX = "/apex/com.android.virt/";
    private static final String INSTANCE_IMG = TEST_ROOT + "instance.img";
    private static final String INSTANCE_ID_FILE = TEST_ROOT + "instance_id";

    private static final int MIN_MEM_ARM64 = 170;
    private static final int MIN_MEM_X86_64 = 196;

    private static final int BOOT_COMPLETE_TIMEOUT = 30000; // 30 seconds

    private static class VmInfo {
        final Process mProcess;

        VmInfo(Process process) {
            mProcess = process;
        }
    }

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

    private String mOs;

    @Rule public TestLogData mTestLogs = new TestLogData();
    @Rule public TestName mTestName = new TestName();
    @Rule public TestMetrics mMetrics = new TestMetrics();

    private String mMetricPrefix;

    private ITestDevice mMicrodroidDevice;

    private int minMemorySize() throws DeviceNotAvailableException {
        CommandRunner android = new CommandRunner(getDevice());
        String abi = android.run("getprop", "ro.product.cpu.abi");
        assertThat(abi).isNotEmpty();
        if (abi.startsWith("arm64")) {
            return MIN_MEM_ARM64;
        } else if (abi.startsWith("x86_64")) {
            return MIN_MEM_X86_64;
        }
        throw new AssertionError("Unsupported ABI: " + abi);
    }

    private static JSONObject newPartition(String label, String path) {
        return new JSONObject(Map.of("label", label, "path", path));
    }

    private void createPayloadMetadata(List<ActiveApexInfo> apexes, File payloadMetadata)
            throws Exception {
        PayloadMetadata.write(
                PayloadMetadata.metadata(
                        "/mnt/apk/assets/vm_config.json",
                        PayloadMetadata.apk("microdroid-apk"),
                        apexes.stream()
                                .map(apex -> PayloadMetadata.apex(apex.name))
                                .collect(toList())),
                payloadMetadata);
    }

    private void resignVirtApex(
            File virtApexDir,
            File signingKey,
            Map<String, File> keyOverrides,
            boolean updateBootconfigs) {
        File signVirtApex = findTestFile("sign_virt_apex");

        RunUtil runUtil = createRunUtil();
        // Set the parent dir on the PATH (e.g. <workdir>/bin)
        String separator = System.getProperty("path.separator");
        String path = signVirtApex.getParentFile().getPath() + separator + System.getenv("PATH");
        runUtil.setEnvVariable("PATH", path);

        List<String> command = new ArrayList<>();
        command.add(signVirtApex.getAbsolutePath());
        if (!updateBootconfigs) {
            command.add("--do_not_update_bootconfigs");
        }
        // In some cases we run a CTS binary that is built from a different branch that the /system
        // image under test. In such cases we might end up in a situation when avb_version used in
        // CTS binary and avb_version used to sign the com.android.virt APEX do not match.
        // This is a weird configuration, but unfortunately it can happen, hence we pass here
        // --do_not_validate_avb_version flag to make sure that CTS doesn't fail on it.
        command.add("--do_not_validate_avb_version");
        keyOverrides.forEach(
                (filename, keyFile) ->
                        command.add("--key_override " + filename + "=" + keyFile.getPath()));
        command.add(signingKey.getPath());
        command.add(virtApexDir.getPath());

        CommandResult result =
                runUtil.runTimedCmd(
                        // sign_virt_apex is so slow on CI server that this often times
                        // out. Until we can make it fast, use 50s for timeout
                        50 * 1000, "/bin/bash", "-c", String.join(" ", command));
        String out = result.getStdout();
        String err = result.getStderr();
        assertWithMessage(
                        "resigning the Virt APEX failed:\n\tout: " + out + "\n\terr: " + err + "\n")
                .about(command_results())
                .that(result)
                .isSuccess();
    }

    private static <T> void assertThatEventually(
            long timeoutMillis, Callable<T> callable, org.hamcrest.Matcher<T> matcher)
            throws Exception {
        long start = System.currentTimeMillis();
        while ((System.currentTimeMillis() - start < timeoutMillis)
                && !matcher.matches(callable.call())) {
            RunUtil.getDefault().sleep(500);
        }
        assertThat(callable.call(), matcher);
    }

    private int getDeviceNumCpus(CommandRunner runner) throws DeviceNotAvailableException {
        return Integer.parseInt(runner.run("nproc --all").trim());
    }

    private int getDeviceNumCpus(ITestDevice device) throws DeviceNotAvailableException {
        return getDeviceNumCpus(new CommandRunner(device));
    }

    static class ActiveApexInfo {
        public String name;
        public String path;
        public boolean provideSharedApexLibs;

        ActiveApexInfo(String name, String path, boolean provideSharedApexLibs) {
            this.name = name;
            this.path = path;
            this.provideSharedApexLibs = provideSharedApexLibs;
        }
    }

    static class ActiveApexInfoList {
        private List<ActiveApexInfo> mList;

        ActiveApexInfoList(List<ActiveApexInfo> list) {
            this.mList = list;
        }

        ActiveApexInfo get(String apexName) {
            return mList.stream()
                    .filter(info -> apexName.equals(info.name))
                    .findFirst()
                    .orElse(null);
        }

        List<ActiveApexInfo> getSharedLibApexes() {
            return mList.stream().filter(info -> info.provideSharedApexLibs).collect(toList());
        }
    }

    private ActiveApexInfoList getActiveApexInfoList() throws Exception {
        String apexInfoListXml = getDevice().pullFileContents("/apex/apex-info-list.xml");
        List<ActiveApexInfo> list = new ArrayList<>();
        new AbstractXmlParser() {
            @Override
            protected DefaultHandler createXmlHandler() {
                return new DefaultHandler() {
                    @Override
                    public void startElement(
                            String uri, String localName, String qName, Attributes attributes) {
                        if (localName.equals("apex-info")
                                && attributes.getValue("isActive").equals("true")) {
                            String name = attributes.getValue("moduleName");
                            String path = attributes.getValue("modulePath");
                            String sharedApex = attributes.getValue("provideSharedApexLibs");
                            list.add(new ActiveApexInfo(name, path, "true".equals(sharedApex)));
                        }
                    }
                };
            }
        }.parse(new ByteArrayInputStream(apexInfoListXml.getBytes()));
        return new ActiveApexInfoList(list);
    }

    private VmInfo runMicrodroidWithResignedImages(
            File key,
            Map<String, File> keyOverrides,
            boolean isProtected,
            boolean updateBootconfigs)
            throws Exception {
        CommandRunner android = new CommandRunner(getDevice());

        File virtApexDir = FileUtil.createTempDir("virt_apex");

        // Pull the virt apex's etc/ directory (which contains images and microdroid.json)
        File virtApexEtcDir = new File(virtApexDir, "etc");
        // We need only etc/ directory for images
        assertWithMessage("Failed to mkdir " + virtApexEtcDir)
                .that(virtApexEtcDir.mkdirs())
                .isTrue();
        assertWithMessage("Failed to pull " + VIRT_APEX + "etc")
                .that(getDevice().pullDir(VIRT_APEX + "etc", virtApexEtcDir))
                .isTrue();

        resignVirtApex(virtApexDir, key, keyOverrides, updateBootconfigs);

        // Push back re-signed virt APEX contents and updated microdroid.json
        getDevice().pushDir(virtApexDir, TEST_ROOT);

        // Create the idsig file for the APK
        final String apkPath = getPathForPackage(PACKAGE_NAME);
        final String idSigPath = TEST_ROOT + "idsig";
        android.run(VIRT_APEX + "bin/vm", "create-idsig", apkPath, idSigPath);

        // Create the instance image for the VM
        final String instanceImgPath = TEST_ROOT + "instance.img";
        android.run(
                VIRT_APEX + "bin/vm",
                "create-partition",
                "--type instance",
                instanceImgPath,
                Integer.toString(10 * 1024 * 1024));

        // payload-metadata is created on device
        final String payloadMetadataPath = TEST_ROOT + "payload-metadata.img";

        // Load /apex/apex-info-list.xml to get paths to APEXes required for the VM.
        ActiveApexInfoList list = getActiveApexInfoList();

        // Since Java APP can't start a VM with a custom image, here, we start a VM using `vm run`
        // command with a VM Raw config which is equiv. to what virtualizationservice creates with
        // a VM App config.
        //
        // 1. use etc/microdroid.json as base
        // 2. add partitions: bootconfig, vbmeta, instance image
        // 3. add a payload image disk with
        //   - payload-metadata
        //   - apexes
        //   - test apk
        //   - its idsig

        // Load etc/microdroid.json
        File microdroidConfigFile = new File(virtApexEtcDir, mOs + ".json");
        JSONObject config = new JSONObject(FileUtil.readStringFromFile(microdroidConfigFile));

        // Replace paths so that the config uses re-signed images from TEST_ROOT
        config.put("kernel", config.getString("kernel").replace(VIRT_APEX, TEST_ROOT));
        JSONArray disks = config.getJSONArray("disks");
        for (int diskIndex = 0; diskIndex < disks.length(); diskIndex++) {
            JSONObject disk = disks.getJSONObject(diskIndex);
            JSONArray partitions = disk.getJSONArray("partitions");
            for (int partIndex = 0; partIndex < partitions.length(); partIndex++) {
                JSONObject part = partitions.getJSONObject(partIndex);
                part.put("path", part.getString("path").replace(VIRT_APEX, TEST_ROOT));
            }
        }

        // Add partitions to the second disk
        final String initrdPath = TEST_ROOT + "etc/" + mOs + "_initrd_debuggable.img";
        config.put("initrd", initrdPath);
        // Add instance image as a partition in disks[1]
        disks.put(
                new JSONObject()
                        .put("writable", true)
                        .put(
                                "partitions",
                                new JSONArray().put(newPartition("vm-instance", instanceImgPath))));
        // Add payload image disk with partitions:
        // - payload-metadata
        // - apexes: com.android.os.statsd, com.android.adbd, [sharedlib apex](optional)
        // - apk and idsig
        List<ActiveApexInfo> apexesForVm = new ArrayList<>();
        apexesForVm.add(list.get("com.android.os.statsd"));
        apexesForVm.add(list.get("com.android.adbd"));
        apexesForVm.addAll(list.getSharedLibApexes());

        final JSONArray partitions = new JSONArray();
        partitions.put(newPartition("payload-metadata", payloadMetadataPath));
        for (ActiveApexInfo apex : apexesForVm) {
            partitions.put(newPartition(apex.name, apex.path));
        }
        partitions
                .put(newPartition("microdroid-apk", apkPath))
                .put(newPartition("microdroid-apk-idsig", idSigPath));
        disks.put(new JSONObject().put("writable", false).put("partitions", partitions));

        final File localPayloadMetadata = new File(virtApexDir, "payload-metadata.img");
        createPayloadMetadata(apexesForVm, localPayloadMetadata);
        getDevice().pushFile(localPayloadMetadata, payloadMetadataPath);

        config.put("protected", isProtected);

        // Write updated raw config
        final String configPath = TEST_ROOT + "raw_config.json";
        getDevice().pushString(config.toString(), configPath);

        List<String> args =
                Arrays.asList(
                        "adb",
                        "-s",
                        getDevice().getSerialNumber(),
                        "shell",
                        VIRT_APEX + "bin/vm run",
                        "--console " + CONSOLE_PATH,
                        "--log " + LOG_PATH,
                        configPath);

        PipedInputStream pis = new PipedInputStream();
        Process process = createRunUtil().runCmdInBackground(args, new PipedOutputStream(pis));
        return new VmInfo(process);
    }

    @Test
    @CddTest
    public void UpgradedPackageIsAcceptedWithSecretkeeper() throws Exception {
        assumeUpdatableVmSupported();
        getDevice().uninstallPackage(PACKAGE_NAME);
        getDevice().installPackage(findTestFile(APK_NAME), /* reinstall= */ true);
        ensureMicrodroidBootsSuccessfully(INSTANCE_ID_FILE, INSTANCE_IMG);

        getDevice().uninstallPackage(PACKAGE_NAME);
        cleanUpVirtualizationTestSetup(getDevice());
        // Install the updated version of app (versionCode 6)
        getDevice().installPackage(findTestFile(APK_UPDATED_NAME), /* reinstall= */ true);
        ensureMicrodroidBootsSuccessfully(INSTANCE_ID_FILE, INSTANCE_IMG);
    }

    @Test
    @CddTest
    public void DowngradedPackageIsRejectedProtectedVm() throws Exception {
        assumeProtectedVm(); // Rollback protection is provided only for protected VM.

        // Install the upgraded version (v6)
        getDevice().uninstallPackage(PACKAGE_NAME);
        getDevice().installPackage(findTestFile(APK_UPDATED_NAME), /* reinstall= */ true);
        ensureMicrodroidBootsSuccessfully(INSTANCE_ID_FILE, INSTANCE_IMG);

        getDevice().uninstallPackage(PACKAGE_NAME);
        cleanUpVirtualizationTestSetup(getDevice());
        // Install the older version (v5)
        getDevice().installPackage(findTestFile(APK_NAME), /* reinstall= */ true);

        assertThrows(
                "pVM must fail to boot with downgraded payload apk",
                DeviceRuntimeException.class,
                () -> ensureMicrodroidBootsSuccessfully(INSTANCE_ID_FILE, INSTANCE_IMG));
    }

    private void ensureMicrodroidBootsSuccessfully(String instanceIdPath, String instanceImgPath)
            throws DeviceNotAvailableException {
        final String configPath = "assets/vm_config.json";
        ITestDevice microdroid = null;
        int timeout = 30000; // 30 seconds
        try {
            microdroid =
                    MicrodroidBuilder.fromDevicePath(getPathForPackage(PACKAGE_NAME), configPath)
                            .debugLevel("full")
                            .memoryMib(minMemorySize())
                            .cpuTopology("match_host")
                            .protectedVm(mProtectedVm)
                            .instanceIdFile(instanceIdPath)
                            .instanceImgFile(instanceImgPath)
                            .setAdbConnectTimeoutMs(timeout)
                            .build(getAndroidDevice());
            assertThat(microdroid.waitForBootComplete(timeout)).isTrue();
            assertThat(microdroid.enableAdbRoot()).isTrue();
        } finally {
            if (microdroid != null) {
                getAndroidDevice().shutdownMicrodroid(microdroid);
            }
        }
    }

    @Test
    @CddTest(requirements = {"9.17/C-2-1", "9.17/C-2-2", "9.17/C-2-6"})
    public void protectedVmRunsPvmfw() throws Exception {
        // Arrange
        assumeProtectedVm();
        final String configPath = "assets/vm_config_apex.json";

        // Act
        mMicrodroidDevice =
                MicrodroidBuilder.fromDevicePath(getPathForPackage(PACKAGE_NAME), configPath)
                        .debugLevel("full")
                        .memoryMib(minMemorySize())
                        .cpuTopology("match_host")
                        .protectedVm(true)
                        .gki(mGki)
                        .name("protected_vm_runs_pvmfw")
                        .build(getAndroidDevice());

        // Assert
        mMicrodroidDevice.waitForBootComplete(BOOT_COMPLETE_TIMEOUT);
        String consoleLog = getDevice().pullFileContents(TRADEFED_CONSOLE_PATH);
        assertWithMessage("Failed to verify that pvmfw started")
                .that(consoleLog)
                .contains("pVM firmware");
        assertWithMessage("pvmfw failed to start kernel")
                .that(consoleLog)
                .contains("Starting payload...");
        // TODO(b/260994818): Investigate the feasibility of checking DeathReason.
    }

    @Test
    @CddTest(requirements = {"9.17/C-2-1", "9.17/C-2-2", "9.17/C-2-6"})
    public void protectedVmWithImageSignedWithDifferentKeyFailsToVerifyPayload() throws Exception {
        // Arrange
        assumeProtectedVm();
        File key = findTestFile("test.com.android.virt.pem");

        // Act
        VmInfo vmInfo =
                runMicrodroidWithResignedImages(
                        key,
                        /* keyOverrides= */ Map.of(),
                        /* isProtected= */ true,
                        /* updateBootconfigs= */ true);

        // Assert
        vmInfo.mProcess.waitFor(5L, TimeUnit.SECONDS);
        String consoleLog = getDevice().pullFileContents(CONSOLE_PATH);
        assertWithMessage("pvmfw should start").that(consoleLog).contains("pVM firmware");
        assertWithMessage("pvmfw should fail to verify the payload")
                .that(consoleLog)
                .contains("Failed to verify the payload");
        vmInfo.mProcess.destroy();
    }

    @Test
    @CddTest(requirements = {"9.17/C-2-2", "9.17/C-2-6"})
    public void testBootSucceedsWhenNonProtectedVmStartsWithImagesSignedWithDifferentKey()
            throws Exception {
        assumeNonProtectedVm();
        File key = findTestFile("test.com.android.virt.pem");
        Map<String, File> keyOverrides = Map.of();
        VmInfo vmInfo =
                runMicrodroidWithResignedImages(
                        key, keyOverrides, /* isProtected= */ false, /* updateBootconfigs= */ true);
        assertThatEventually(
                100000,
                () ->
                        getDevice().pullFileContents(CONSOLE_PATH)
                                + getDevice().pullFileContents(LOG_PATH),
                containsString("boot completed, time to run payload"));

        vmInfo.mProcess.destroy();
    }

    @Test
    @CddTest(requirements = {"9.17/C-2-2", "9.17/C-2-6"})
    public void testBootFailsWhenVbMetaDigestDoesNotMatchBootconfig() throws Exception {
        // protectedVmWithImageSignedWithDifferentKeyRunsPvmfw() is the protected case.
        assumeNonProtectedVm();
        // Sign everything with key1 except vbmeta
        File key = findTestFile("test.com.android.virt.pem");
        // To be able to stop it, it should be a daemon.
        VmInfo vmInfo =
                runMicrodroidWithResignedImages(
                        key, Map.of(), /* isProtected= */ false, /* updateBootconfigs= */ false);
        // Wait so that init can print errors to console (time in cuttlefish >> in real device)
        assertThatEventually(
                100000,
                () -> getDevice().pullFileContents(CONSOLE_PATH),
                containsString("init: [libfs_avb] Failed to verify vbmeta digest"));
        vmInfo.mProcess.destroy();
    }

    private void waitForCrosvmExit(CommandRunner android, String testStartTime) throws Exception {
        // TODO: improve crosvm exit check. b/258848245
        android.runWithTimeout(
                15000,
                "logcat",
                "-m",
                "1",
                "-e",
                "'virtualizationmanager::crosvm.*exited with status exit status:'",
                "-T",
                "'" + testStartTime + "'");
    }

    private boolean isTombstoneReceivedFromHostLogcat(String testStartTime) throws Exception {
        // Note this method relies on logcat values being printed by the receiver on host
        // userspace crash log: virtualizationservice/src/aidl.rs
        // kernel ramdump log: virtualizationmanager/src/crosvm.rs
        String ramdumpRegex =
                "Received [0-9]+ bytes from guest & wrote to tombstone file|"
                        + "Ramdump \"[^ ]+/ramdump\" sent to tombstoned";

        String result =
                tryRunOnHost(
                        "timeout",
                        "3s",
                        "adb",
                        "-s",
                        getDevice().getSerialNumber(),
                        "logcat",
                        "-m",
                        "1",
                        "-e",
                        ramdumpRegex,
                        "-T",
                        testStartTime);
        return !result.trim().isEmpty();
    }

    private boolean isTombstoneGeneratedWithCmd(
            boolean protectedVm, String configPath, String... crashCommand) throws Exception {
        CommandRunner android = new CommandRunner(getDevice());
        String testStartTime = android.runWithTimeout(1000, "date", "'+%Y-%m-%d %H:%M:%S.%N'");

        mMicrodroidDevice =
                MicrodroidBuilder.fromDevicePath(getPathForPackage(PACKAGE_NAME), configPath)
                        .debugLevel("full")
                        .memoryMib(minMemorySize())
                        .cpuTopology("match_host")
                        .protectedVm(protectedVm)
                        .gki(mGki)
                        .build(getAndroidDevice());
        mMicrodroidDevice.waitForBootComplete(BOOT_COMPLETE_TIMEOUT);
        mMicrodroidDevice.enableAdbRoot();

        CommandRunner microdroid = new CommandRunner(mMicrodroidDevice);
        // can crash in the middle of crashCommand; fail is ok
        microdroid.tryRun(crashCommand);

        // check until microdroid is shut down
        waitForCrosvmExit(android, testStartTime);

        return isTombstoneReceivedFromHostLogcat(testStartTime);
    }

    @Test
    public void testTombstonesAreGeneratedUponUserspaceCrash() throws Exception {
        // TODO(b/291867858): tombstones are failing in HWASAN enabled Microdroid.
        assumeFalse("tombstones are failing in HWASAN enabled Microdroid.", isHwasan());
        assertThat(
                        isTombstoneGeneratedWithCmd(
                                mProtectedVm,
                                "assets/vm_config.json",
                                "kill",
                                "-SIGSEGV",
                                "$(pidof microdroid_launcher)"))
                .isTrue();
    }

    @Test
    public void testTombstonesAreNotGeneratedIfNotExportedUponUserspaceCrash() throws Exception {
        // TODO(b/291867858): tombstones are failing in HWASAN enabled Microdroid.
        assumeFalse("tombstones are failing in HWASAN enabled Microdroid.", isHwasan());
        assertThat(
                        isTombstoneGeneratedWithCmd(
                                mProtectedVm,
                                "assets/vm_config_no_tombstone.json",
                                "kill",
                                "-SIGSEGV",
                                "$(pidof microdroid_launcher)"))
                .isFalse();
    }

    @Test
    @Ignore("b/341087884") // TODO(b/341087884): fix & re-enable
    public void testTombstonesAreGeneratedUponKernelCrash() throws Exception {
        assumeFalse("Cuttlefish is not supported", isCuttlefish());
        assumeFalse("Skipping test because ramdump is disabled on user build", isUserBuild());
        assertThat(
                        isTombstoneGeneratedWithCmd(
                                mProtectedVm,
                                "assets/vm_config.json",
                                "echo",
                                "c",
                                ">",
                                "/proc/sysrq-trigger"))
                .isTrue();
    }

    private boolean isTombstoneGeneratedWithVmRunApp(
            boolean protectedVm, boolean debuggable, String... additionalArgs) throws Exception {
        // we can't use microdroid builder as it wants ADB connection (debuggable)
        CommandRunner android = new CommandRunner(getDevice());
        String testStartTime = android.runWithTimeout(1000, "date", "'+%Y-%m-%d %H:%M:%S.%N'");

        android.run("rm", "-rf", TEST_ROOT + "*");
        android.run("mkdir", "-p", TEST_ROOT + "*");

        final String apkPath = getPathForPackage(PACKAGE_NAME);
        final String idsigPath = TEST_ROOT + "idsig";
        final String instanceImgPath = TEST_ROOT + "instance.img";
        final String instanceIdPath = TEST_ROOT + "instance_id";
        List<String> cmd =
                new ArrayList<>(
                        Arrays.asList(
                                VIRT_APEX + "bin/vm",
                                "run-app",
                                "--debug",
                                debuggable ? "full" : "none",
                                apkPath,
                                idsigPath,
                                instanceImgPath));
        if (isFeatureEnabled("com.android.kvm.LLPVM_CHANGES")) {
            cmd.add("--instance-id-file");
            cmd.add(instanceIdPath);
        }
        ;
        if (protectedVm) {
            cmd.add("--protected");
        }
        if (mGki != null) {
            cmd.add("--gki");
            cmd.add(mGki);
        }
        Collections.addAll(cmd, additionalArgs);

        android.run(cmd.toArray(new String[0]));
        return isTombstoneReceivedFromHostLogcat(testStartTime);
    }

    private boolean isTombstoneGeneratedWithCrashPayload(boolean protectedVm, boolean debuggable)
            throws Exception {
        return isTombstoneGeneratedWithVmRunApp(
                protectedVm, debuggable, "--payload-binary-name", "MicrodroidCrashNativeLib.so");
    }

    @Test
    public void testTombstonesAreGeneratedWithCrashPayload() throws Exception {
        // TODO(b/291867858): tombstones are failing in HWASAN enabled Microdroid.
        assumeFalse("tombstones are failing in HWASAN enabled Microdroid.", isHwasan());
        assertThat(isTombstoneGeneratedWithCrashPayload(mProtectedVm, /* debuggable= */ true))
                .isTrue();
    }

    @Test
    public void testTombstonesAreNotGeneratedWithCrashPayloadWhenNonDebuggable() throws Exception {
        // TODO(b/291867858): tombstones are failing in HWASAN enabled Microdroid.
        assumeFalse("tombstones are failing in HWASAN enabled Microdroid.", isHwasan());
        assertThat(isTombstoneGeneratedWithCrashPayload(mProtectedVm, /* debuggable= */ false))
                .isFalse();
    }

    private boolean isTombstoneGeneratedWithCrashConfig(boolean protectedVm, boolean debuggable)
            throws Exception {
        return isTombstoneGeneratedWithVmRunApp(
                protectedVm, debuggable, "--config-path", "assets/vm_config_crash.json");
    }

    @Test
    public void testTombstonesAreGeneratedWithCrashConfig() throws Exception {
        // TODO(b/291867858): tombstones are failing in HWASAN enabled Microdroid.
        assumeFalse("tombstones are failing in HWASAN enabled Microdroid.", isHwasan());
        assertThat(isTombstoneGeneratedWithCrashConfig(mProtectedVm, /* debuggable= */ true))
                .isTrue();
    }

    @Test
    public void testTombstonesAreNotGeneratedWithCrashConfigWhenNonDebuggable() throws Exception {
        // TODO(b/291867858): tombstones are failing in HWASAN enabled Microdroid.
        assumeFalse("tombstones are failing in HWASAN enabled Microdroid.", isHwasan());
        assertThat(isTombstoneGeneratedWithCrashConfig(mProtectedVm, /* debuggable= */ false))
                .isFalse();
    }

    @Test
    public void testTelemetryPushedAtoms() throws Exception {
        // Reset statsd config and report before the test
        ConfigUtils.removeConfig(getDevice());
        ReportUtils.clearReports(getDevice());

        // Setup statsd config
        int[] atomIds = {
            AtomsProto.Atom.VM_CREATION_REQUESTED_FIELD_NUMBER,
            AtomsProto.Atom.VM_BOOTED_FIELD_NUMBER,
            AtomsProto.Atom.VM_EXITED_FIELD_NUMBER,
        };
        ConfigUtils.uploadConfigForPushedAtoms(getDevice(), PACKAGE_NAME, atomIds);

        // Create VM with microdroid
        TestDevice device = getAndroidDevice();
        final String configPath = "assets/vm_config_apex.json"; // path inside the APK
        ITestDevice microdroid =
                MicrodroidBuilder.fromDevicePath(getPathForPackage(PACKAGE_NAME), configPath)
                        .debugLevel("full")
                        .memoryMib(minMemorySize())
                        .cpuTopology("match_host")
                        .protectedVm(mProtectedVm)
                        .gki(mGki)
                        .name("test_telemetry_pushed_atoms")
                        .build(device);
        microdroid.waitForBootComplete(BOOT_COMPLETE_TIMEOUT);
        device.shutdownMicrodroid(microdroid);

        // Try to collect atoms for 60000 milliseconds.
        List<StatsLog.EventMetricData> data = new ArrayList<>();
        long start = System.currentTimeMillis();
        while ((System.currentTimeMillis() - start < 60000) && data.size() < 3) {
            data.addAll(ReportUtils.getEventMetricDataList(getDevice()));
            Thread.sleep(500);
        }
        assertThat(
                        data.stream()
                                .map(x -> x.getAtom().getPushedCase().getNumber())
                                .collect(Collectors.toList()))
                .containsExactly(
                        AtomsProto.Atom.VM_CREATION_REQUESTED_FIELD_NUMBER,
                        AtomsProto.Atom.VM_BOOTED_FIELD_NUMBER,
                        AtomsProto.Atom.VM_EXITED_FIELD_NUMBER)
                .inOrder();

        // Check VmCreationRequested atom
        AtomsProto.VmCreationRequested atomVmCreationRequested =
                data.get(0).getAtom().getVmCreationRequested();
        if (isPkvmHypervisor()) {
            assertThat(atomVmCreationRequested.getHypervisor())
                    .isEqualTo(AtomsProto.VmCreationRequested.Hypervisor.PKVM);
        }
        assertThat(atomVmCreationRequested.getIsProtected()).isEqualTo(mProtectedVm);
        assertThat(atomVmCreationRequested.getCreationSucceeded()).isTrue();
        assertThat(atomVmCreationRequested.getBinderExceptionCode()).isEqualTo(0);
        assertThat(atomVmCreationRequested.getVmIdentifier())
                .isEqualTo("test_telemetry_pushed_atoms");
        assertThat(atomVmCreationRequested.getConfigType())
                .isEqualTo(AtomsProto.VmCreationRequested.ConfigType.VIRTUAL_MACHINE_APP_CONFIG);
        assertThat(atomVmCreationRequested.getNumCpus()).isEqualTo(getDeviceNumCpus(device));
        assertThat(atomVmCreationRequested.getMemoryMib()).isEqualTo(minMemorySize());
        assertThat(atomVmCreationRequested.getApexes())
                .isEqualTo("com.android.art:com.android.compos:com.android.sdkext");

        // Check VmBooted atom
        AtomsProto.VmBooted atomVmBooted = data.get(1).getAtom().getVmBooted();
        assertThat(atomVmBooted.getVmIdentifier()).isEqualTo("test_telemetry_pushed_atoms");

        // Check VmExited atom
        AtomsProto.VmExited atomVmExited = data.get(2).getAtom().getVmExited();
        assertThat(atomVmExited.getVmIdentifier()).isEqualTo("test_telemetry_pushed_atoms");
        assertThat(atomVmExited.getDeathReason()).isEqualTo(AtomsProto.VmExited.DeathReason.KILLED);
        assertThat(atomVmExited.getExitSignal()).isEqualTo(9);
        // In CPU & memory related fields, check whether positive values are collected or not.
        if (isPkvmHypervisor()) {
            // Guest Time may not be updated on other hypervisors.
            // Checking only if the hypervisor is PKVM.
            assertThat(atomVmExited.getGuestTimeMillis()).isGreaterThan(0);
        }
        assertThat(atomVmExited.getRssVmKb()).isGreaterThan(0);
        assertThat(atomVmExited.getRssCrosvmKb()).isGreaterThan(0);

        // Check UID and elapsed_time by comparing each other.
        assertThat(atomVmBooted.getUid()).isEqualTo(atomVmCreationRequested.getUid());
        assertThat(atomVmExited.getUid()).isEqualTo(atomVmCreationRequested.getUid());
        assertThat(atomVmBooted.getElapsedTimeMillis())
                .isLessThan(atomVmExited.getElapsedTimeMillis());
    }

    private void testMicrodroidBootsWithBuilder(MicrodroidBuilder builder) throws Exception {
        CommandRunner android = new CommandRunner(getDevice());

        mMicrodroidDevice = builder.build(getAndroidDevice());
        mMicrodroidDevice.waitForBootComplete(BOOT_COMPLETE_TIMEOUT);
        CommandRunner microdroid = new CommandRunner(mMicrodroidDevice);

        String vmList = android.run("/apex/com.android.virt/bin/vm list");
        assertThat(vmList).contains("requesterUid: " + android.run("id -u"));

        // Test writing to /data partition
        microdroid.run("echo MicrodroidTest > /data/local/tmp/test.txt");
        assertThat(microdroid.run("cat /data/local/tmp/test.txt")).isEqualTo("MicrodroidTest");

        // Check if the APK & its idsig partitions exist
        final String apkPartition = "/dev/block/by-name/microdroid-apk";
        assertThat(microdroid.run("ls", apkPartition)).isEqualTo(apkPartition);
        final String apkIdsigPartition = "/dev/block/by-name/microdroid-apk-idsig";
        assertThat(microdroid.run("ls", apkIdsigPartition)).isEqualTo(apkIdsigPartition);
        // Check the vm-instance partition as well
        final String vmInstancePartition = "/dev/block/by-name/vm-instance";
        assertThat(microdroid.run("ls", vmInstancePartition)).isEqualTo(vmInstancePartition);

        // Check if the native library in the APK is has correct filesystem info
        final String[] abis = microdroid.run("getprop", "ro.product.cpu.abilist").split(",");
        assertWithMessage("Incorrect ABI list").that(abis).hasLength(1);

        // Check that no denials have happened so far
        String consoleText = getDevice().pullFileContents(TRADEFED_CONSOLE_PATH);
        assertWithMessage("Console output shouldn't be empty").that(consoleText).isNotEmpty();
        String logText = getDevice().pullFileContents(TRADEFED_LOG_PATH);
        assertWithMessage("Log output shouldn't be empty").that(logText).isNotEmpty();

        assertWithMessage("Unexpected denials during VM boot")
                .that(consoleText + logText)
                .doesNotContainMatch("avc:\\s+denied");

        assertThat(getDeviceNumCpus(microdroid)).isEqualTo(getDeviceNumCpus(android));

        // Check that selinux is enabled
        assertWithMessage("SELinux should be in enforcing mode")
                .that(microdroid.run("getenforce"))
                .isEqualTo("Enforcing");

        // TODO(b/176805428): adb is broken for nested VM
        if (!isCuttlefish()) {
            // Check neverallow rules on microdroid
            File policyFile = mMicrodroidDevice.pullFile("/sys/fs/selinux/policy");
            File generalPolicyConfFile = findTestFile("microdroid_general_sepolicy.conf");
            File sepolicyAnalyzeBin = findTestFile("sepolicy-analyze");

            CommandResult result =
                    createRunUtil()
                            .runTimedCmd(
                                    10000,
                                    sepolicyAnalyzeBin.getPath(),
                                    policyFile.getPath(),
                                    "neverallow",
                                    "-w",
                                    "-f",
                                    generalPolicyConfFile.getPath());
            assertWithMessage("neverallow check failed: " + result.getStderr().trim())
                    .about(command_results())
                    .that(result)
                    .isSuccess();
        }
    }

    @Test
    @CddTest(requirements = {"9.17/C-1-1", "9.17/C-1-2", "9.17/C/1-3"})
    public void testMicrodroidBoots() throws Exception {
        final String configPath = "assets/vm_config.json"; // path inside the APK
        testMicrodroidBootsWithBuilder(
                MicrodroidBuilder.fromDevicePath(getPathForPackage(PACKAGE_NAME), configPath)
                        .debugLevel("full")
                        .memoryMib(minMemorySize())
                        .cpuTopology("match_host")
                        .protectedVm(mProtectedVm)
                        .name("test_microdroid_boots")
                        .gki(mGki));
    }

    @Test
    public void testMicrodroidRamUsage() throws Exception {
        final String configPath = "assets/vm_config.json";
        mMicrodroidDevice =
                MicrodroidBuilder.fromDevicePath(getPathForPackage(PACKAGE_NAME), configPath)
                        .debugLevel("full")
                        .memoryMib(minMemorySize())
                        .cpuTopology("match_host")
                        .protectedVm(mProtectedVm)
                        .gki(mGki)
                        .name("test_microdroid_ram_usage")
                        .build(getAndroidDevice());
        mMicrodroidDevice.waitForBootComplete(BOOT_COMPLETE_TIMEOUT);
        mMicrodroidDevice.enableAdbRoot();

        CommandRunner microdroid = new CommandRunner(mMicrodroidDevice);
        Function<String, String> microdroidExec =
                (cmd) -> {
                    try {
                        return microdroid.run(cmd);
                    } catch (Exception ex) {
                        throw new IllegalStateException(ex);
                    }
                };

        for (Map.Entry<String, Long> stat :
                ProcessUtil.getProcessMemoryMap(microdroidExec).entrySet()) {
            mMetrics.addTestMetric(
                    mMetricPrefix + "meminfo/" + stat.getKey().toLowerCase(),
                    stat.getValue().toString());
        }

        for (Map.Entry<Integer, String> proc :
                ProcessUtil.getProcessMap(microdroidExec).entrySet()) {
            for (Map.Entry<String, Long> stat :
                    ProcessUtil.getProcessSmapsRollup(proc.getKey(), microdroidExec).entrySet()) {
                String name = stat.getKey().toLowerCase();
                mMetrics.addTestMetric(
                        mMetricPrefix + "smaps/" + name + "/" + proc.getValue(),
                        stat.getValue().toString());
            }
        }
    }

    @Test
    public void testPathToBinaryIsRejected() throws Exception {
        CommandRunner android = new CommandRunner(getDevice());

        // Create the idsig file for the APK
        final String apkPath = getPathForPackage(PACKAGE_NAME);
        final String idSigPath = TEST_ROOT + "idsig";
        android.run(VIRT_APEX + "bin/vm", "create-idsig", apkPath, idSigPath);
        // Create the instance image for the VM
        final String instanceImgPath = TEST_ROOT + "instance.img";
        android.run(
                VIRT_APEX + "bin/vm",
                "create-partition",
                "--type instance",
                instanceImgPath,
                Integer.toString(10 * 1024 * 1024));

        List<String> cmd =
                new ArrayList<>(
                        Arrays.asList(
                                VIRT_APEX + "bin/vm",
                                "run-app",
                                "--payload-binary-name",
                                "./MicrodroidTestNativeLib.so",
                                apkPath,
                                idSigPath,
                                instanceImgPath));
        if (isFeatureEnabled("com.android.kvm.LLPVM_CHANGES")) {
            cmd.add("--instance-id-file");
            cmd.add(TEST_ROOT + "instance_id");
        }

        final String ret = android.runForResult(String.join(" ", cmd)).getStderr().trim();

        assertThat(ret).contains("Payload binary name must not specify a path");
    }

    @Test
    @CddTest(requirements = {"9.17/C-2-2", "9.17/C-2-6"})
    public void testAllVbmetaUseSHA256() throws Exception {
        File virtApexDir = FileUtil.createTempDir("virt_apex");
        // Pull the virt apex's etc/ directory (which contains images)
        File virtApexEtcDir = new File(virtApexDir, "etc");
        // We need only etc/ directory for images
        assertWithMessage("Failed to mkdir " + virtApexEtcDir)
                .that(virtApexEtcDir.mkdirs())
                .isTrue();
        assertWithMessage("Failed to pull " + VIRT_APEX + "etc")
                .that(getDevice().pullDir(VIRT_APEX + "etc", virtApexEtcDir))
                .isTrue();

        checkHashAlgorithm(virtApexEtcDir);
    }

    @Test
    @CddTest
    public void testNoAvfDebugPolicyInLockedDevice() throws Exception {
        ITestDevice device = getDevice();

        // Check device's locked state with ro.boot.verifiedbootstate. ro.boot.flash.locked
        // may not be set if ro.oem_unlock_supported is false.
        String lockProp = device.getProperty("ro.boot.verifiedbootstate");
        assumeFalse("Unlocked devices may have AVF debug policy", lockProp.equals("orange"));

        // Test that AVF debug policy doesn't exist.
        boolean hasDebugPolicy = device.doesFileExist("/proc/device-tree/avf/guest");
        assertThat(hasDebugPolicy).isFalse();
    }

    private boolean isLz4(String path) throws Exception {
        File lz4tool = findTestFile("lz4");
        CommandResult result =
                createRunUtil().runTimedCmd(5000, lz4tool.getAbsolutePath(), "-t", path);
        return result.getStatus() == CommandStatus.SUCCESS;
    }

    private void decompressLz4(String inputPath, String outputPath) throws Exception {
        File lz4tool = findTestFile("lz4");
        CommandResult result =
                createRunUtil()
                        .runTimedCmd(
                                5000, lz4tool.getAbsolutePath(), "-d", "-f", inputPath, outputPath);
        String out = result.getStdout();
        String err = result.getStderr();
        assertWithMessage(
                        "lz4 image "
                                + inputPath
                                + " decompression failed."
                                + "\n\tout: "
                                + out
                                + "\n\terr: "
                                + err
                                + "\n")
                .about(command_results())
                .that(result)
                .isSuccess();
    }

    private String avbInfo(String image_path) throws Exception {
        if (isLz4(image_path)) {
            File decompressedImage = FileUtil.createTempFile("decompressed", ".img");
            decompressedImage.deleteOnExit();
            decompressLz4(image_path, decompressedImage.getAbsolutePath());
            image_path = decompressedImage.getAbsolutePath();
        }

        File avbtool = findTestFile("avbtool");
        List<String> command =
                Arrays.asList(avbtool.getAbsolutePath(), "info_image", "--image", image_path);
        CommandResult result =
                createRunUtil().runTimedCmd(5000, "/bin/bash", "-c", String.join(" ", command));
        String out = result.getStdout();
        String err = result.getStderr();
        assertWithMessage(
                        "Command "
                                + command
                                + " failed."
                                + ":\n\tout: "
                                + out
                                + "\n\terr: "
                                + err
                                + "\n")
                .about(command_results())
                .that(result)
                .isSuccess();
        return out;
    }

    private void checkHashAlgorithm(File virtApexEtcDir) throws Exception {
        List<String> images =
                Arrays.asList(
                        // kernel image (contains descriptors from initrd(s) as well)
                        "/fs/microdroid_kernel",
                        // vbmeta partition (contains descriptors from vendor/system images)
                        "/fs/microdroid_vbmeta.img");

        for (String path : images) {
            String info = avbInfo(virtApexEtcDir + path);
            Pattern pattern = Pattern.compile("Hash Algorithm:[ ]*(sha1|sha256)");
            Matcher m = pattern.matcher(info);
            while (m.find()) {
                assertThat(m.group(1)).isEqualTo("sha256");
            }
        }
    }

    @Test
    public void testDeviceAssignment() throws Exception {
        // Check for preconditions
        assumeVfioPlatformSupported();

        List<AssignableDevice> devices = getAssignableDevices();
        assumeFalse("no assignable devices", devices.isEmpty());

        String dtSysfsPath = "/proc/device-tree/";

        // Try assign devices one by one
        for (AssignableDevice device : devices) {
            launchWithDeviceAssignment(device.node);

            String dtPath =
                    new CommandRunner(mMicrodroidDevice)
                            .run("cat", dtSysfsPath + "__symbols__/" + device.dtbo_label);
            assertThat(dtPath).isNotEmpty();

            String resolvedDtPath =
                    new CommandRunner(mMicrodroidDevice)
                            .run("readlink", "-e", dtSysfsPath + dtPath);
            assertThat(resolvedDtPath).isNotEmpty();

            String allDevices =
                    new CommandRunner(mMicrodroidDevice)
                            .run("readlink", "-e", "/sys/bus/platform/devices/*/of_node");
            assertThat(allDevices.split("\n")).asList().contains(resolvedDtPath);

            getAndroidDevice().shutdownMicrodroid(mMicrodroidDevice);
            mMicrodroidDevice = null;
        }
    }

    private void launchWithDeviceAssignment(String device) throws Exception {
        Objects.requireNonNull(device);
        final String configPath = "assets/vm_config.json";

        mMicrodroidDevice =
                MicrodroidBuilder.fromDevicePath(getPathForPackage(PACKAGE_NAME), configPath)
                        .debugLevel("full")
                        .memoryMib(minMemorySize())
                        .cpuTopology("match_host")
                        .protectedVm(mProtectedVm)
                        .gki(mGki)
                        .addAssignableDevice(device)
                        .build(getAndroidDevice());

        assertThat(mMicrodroidDevice.waitForBootComplete(BOOT_COMPLETE_TIMEOUT)).isTrue();
        assertThat(mMicrodroidDevice.enableAdbRoot()).isTrue();
    }

    @Test
    public void testGkiVersions() throws Exception {
        for (String gki : getSupportedGKIVersions()) {
            assertTrue(
                    "Unknown gki \"" + gki + "\". Supported gkis: " + SUPPORTED_GKI_VERSIONS,
                    SUPPORTED_GKI_VERSIONS.contains(gki));
        }
    }

    @Test
    public void testHugePages() throws Exception {
        ITestDevice device = getDevice();
        boolean disableRoot = !device.isAdbRoot();
        CommandRunner android = new CommandRunner(device);

        final String SHMEM_ENABLED_PATH = "/sys/kernel/mm/transparent_hugepage/shmem_enabled";
        String thpShmemStr = android.run("cat", SHMEM_ENABLED_PATH);

        assumeFalse("shmem already enabled, skip", thpShmemStr.contains("[advise]"));
        assumeTrue("Unsupported shmem, skip", thpShmemStr.contains("[never]"));

        device.enableAdbRoot();
        assumeTrue("adb root is not enabled", device.isAdbRoot());
        android.run("echo advise > " + SHMEM_ENABLED_PATH);

        final String configPath = "assets/vm_config.json";
        mMicrodroidDevice =
                MicrodroidBuilder.fromDevicePath(getPathForPackage(PACKAGE_NAME), configPath)
                        .debugLevel("full")
                        .memoryMib(minMemorySize())
                        .cpuTopology("match_host")
                        .protectedVm(mProtectedVm)
                        .gki(mGki)
                        .hugePages(true)
                        .name("test_huge_pages")
                        .build(getAndroidDevice());
        mMicrodroidDevice.waitForBootComplete(BOOT_COMPLETE_TIMEOUT);

        android.run("echo never >" + SHMEM_ENABLED_PATH);
        if (disableRoot) {
            device.disableAdbRoot();
        }
    }

    @Before
    public void setUp() throws Exception {
        assumeDeviceIsCapable(getDevice());
        mMetricPrefix = getMetricPrefix() + "microdroid/";
        mMicrodroidDevice = null;

        prepareVirtualizationTestSetup(getDevice());

        getDevice().installPackage(findTestFile(APK_NAME), /* reinstall= */ false);

        // Skip test if given device doesn't support protected or non-protected VM.
        assumeTrue(
                "Microdroid is not supported for specific VM protection type",
                getAndroidDevice().supportsMicrodroid(mProtectedVm));

        if (mGki != null) {
            assumeTrue(
                    "GKI version \"" + mGki + "\" is not supported on this device",
                    getSupportedGKIVersions().contains(mGki));
        }

        mOs = (mGki != null) ? "microdroid_gki-" + mGki : "microdroid";

        new CommandRunner(getDevice())
                .tryRun(
                        "pm",
                        "grant",
                        SHELL_PACKAGE_NAME,
                        "android.permission.USE_CUSTOM_VIRTUAL_MACHINE");
    }

    @After
    public void shutdown() throws Exception {
        if (mMicrodroidDevice != null) {
            getAndroidDevice().shutdownMicrodroid(mMicrodroidDevice);
        }

        cleanUpVirtualizationTestSetup(getDevice());

        archiveLogThenDelete(
                mTestLogs, getDevice(), LOG_PATH, "vm.log-" + mTestName.getMethodName());

        getDevice().uninstallPackage(PACKAGE_NAME);
    }

    private void assumeProtectedVm() {
        assumeTrue("This test is only for protected VM.", mProtectedVm);
    }

    private void assumeNonProtectedVm() {
        assumeFalse("This test is only for non-protected VM.", mProtectedVm);
    }

    private void assumeVfioPlatformSupported() throws Exception {
        TestDevice device = getAndroidDevice();
        assumeTrue(
                "Test skipped because VFIO platform is not supported.",
                device.doesFileExist("/dev/vfio/vfio")
                        && device.doesFileExist("/sys/bus/platform/drivers/vfio-platform"));
    }

    private void assumeUpdatableVmSupported() throws DeviceNotAvailableException {
        assumeTrue(
                "This test is only applicable if if Updatable VMs are supported",
                isUpdatableVmSupported());
    }

    private TestDevice getAndroidDevice() {
        TestDevice androidDevice = (TestDevice) getDevice();
        assertThat(androidDevice).isNotNull();
        return androidDevice;
    }

    // The TradeFed Dockerfile sets LD_LIBRARY_PATH to a directory with an older libc++.so, which
    // breaks binaries that are linked against a newer libc++.so. Binaries commonly use DT_RUNPATH
    // to find an adjacent libc++.so (e.g. `$ORIGIN/../lib64`), but LD_LIBRARY_PATH overrides
    // DT_RUNPATH, so clear LD_LIBRARY_PATH. See b/332593805 and b/333782216.
    private static RunUtil createRunUtil() {
        RunUtil runUtil = new RunUtil();
        runUtil.unsetEnvVariable("LD_LIBRARY_PATH");
        return runUtil;
    }
}
