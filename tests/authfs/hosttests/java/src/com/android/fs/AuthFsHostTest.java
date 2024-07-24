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

package com.android.virt.fs;

import static com.android.microdroid.test.host.CommandResultSubject.assertThat;

import static com.google.common.truth.Truth.assertThat;

import static org.junit.Assert.assertEquals;
import static org.junit.Assume.assumeTrue;

import android.platform.test.annotations.RootPermissionTest;

import com.android.fs.common.AuthFsTestRule;
import com.android.microdroid.test.host.CommandRunner;
import com.android.tradefed.device.DeviceNotAvailableException;
import com.android.tradefed.invoker.TestInformation;
import com.android.tradefed.log.LogUtil.CLog;
import com.android.tradefed.testtype.DeviceJUnit4ClassRunner;
import com.android.tradefed.testtype.junit4.AfterClassWithInfo;
import com.android.tradefed.testtype.junit4.BaseHostJUnit4Test;
import com.android.tradefed.testtype.junit4.BeforeClassWithInfo;
import com.android.tradefed.util.CommandResult;

import org.junit.Rule;
import org.junit.Test;
import org.junit.runner.RunWith;

@RootPermissionTest
@RunWith(DeviceJUnit4ClassRunner.class)
public final class AuthFsHostTest extends BaseHostJUnit4Test {

    /** Test directory on Android where data are located */
    private static final String TEST_DIR = AuthFsTestRule.TEST_DIR;

    /** Output directory where the test can generate output on Android */
    private static final String TEST_OUTPUT_DIR = AuthFsTestRule.TEST_OUTPUT_DIR;

    /** Path to fsverity on Android */
    private static final String FSVERITY_BIN = "/data/local/tmp/fsverity";

    /** Mount point of authfs on Microdroid during the test */
    private static final String MOUNT_DIR = AuthFsTestRule.MOUNT_DIR;

    /** Input manifest path in the VM. */
    private static final String INPUT_MANIFEST_PATH = "/mnt/apk/assets/input_manifest.pb";

    // fs-verity digest (sha256) of testdata/input.{4k, 4k1, 4m}
    private static final String DIGEST_4K =
            "sha256-9828cd65f4744d6adda216d3a63d8205375be485bfa261b3b8153d3358f5a576";
    private static final String DIGEST_4K1 =
            "sha256-3c70dcd4685ed256ebf1ef116c12e472f35b5017eaca422c0483dadd7d0b5a9f";
    private static final String DIGEST_4M =
            "sha256-f18a268d565348fb4bbf11f10480b198f98f2922eb711de149857b3cecf98a8d";

    private static CommandRunner sAndroid;
    private static CommandRunner sMicrodroid;

    @Rule public final AuthFsTestRule mAuthFsTestRule = new AuthFsTestRule();

    @BeforeClassWithInfo
    public static void beforeClassWithDevice(TestInformation testInfo) throws Exception {
        AuthFsTestRule.setUpAndroid(testInfo);
        assumeTrue(AuthFsTestRule.getDevice().supportsMicrodroid(/* protectedVm= */ true));
        AuthFsTestRule.startMicrodroid(/* protectedVm= */ true);
        sAndroid = AuthFsTestRule.getAndroid();
        sMicrodroid = AuthFsTestRule.getMicrodroid();
    }

    @AfterClassWithInfo
    public static void afterClassWithDevice(TestInformation testInfo)
            throws DeviceNotAvailableException {
        AuthFsTestRule.shutdownMicrodroid();
        AuthFsTestRule.tearDownAndroid();
    }

    @Test
    public void testReadWithFsverityVerification_RemoteFile() throws Exception {
        // Setup
        runFdServerOnAndroid(
                "--open-ro 3:input.4m --open-ro 4:input.4m.fsv_meta --open-ro 6:input.4m",
                "--ro-fds 3:4 --ro-fds 6");
        runAuthFsOnMicrodroid("--remote-ro-file-unverified 6 --remote-ro-file 3:" + DIGEST_4M);

        // Action
        String actualHashUnverified4m = computeFileHash(sMicrodroid, MOUNT_DIR + "/6");
        String actualHash4m = computeFileHash(sMicrodroid, MOUNT_DIR + "/3");

        // Verify
        String expectedHash4m = computeFileHash(sAndroid, TEST_DIR + "/input.4m");

        assertEquals("Inconsistent hash from /authfs/6: ", expectedHash4m, actualHashUnverified4m);
        assertEquals("Inconsistent hash from /authfs/3: ", expectedHash4m, actualHash4m);
    }

    // Separate the test from the above simply because exec in shell does not allow open too many
    // files.
    @Test
    public void testReadWithFsverityVerification_RemoteSmallerFile() throws Exception {
        // Setup
        runFdServerOnAndroid(
                "--open-ro 3:input.4k --open-ro 4:input.4k.fsv_meta --open-ro"
                        + " 6:input.4k1 --open-ro 7:input.4k1.fsv_meta",
                "--ro-fds 3:4 --ro-fds 6:7");
        runAuthFsOnMicrodroid(
                "--remote-ro-file 3:" + DIGEST_4K + " --remote-ro-file 6:" + DIGEST_4K1);

        // Action
        String actualHash4k = computeFileHash(sMicrodroid, MOUNT_DIR + "/3");
        String actualHash4k1 = computeFileHash(sMicrodroid, MOUNT_DIR + "/6");

        // Verify
        String expectedHash4k = computeFileHash(sAndroid, TEST_DIR + "/input.4k");
        String expectedHash4k1 = computeFileHash(sAndroid, TEST_DIR + "/input.4k1");

        assertEquals("Inconsistent hash from /authfs/3: ", expectedHash4k, actualHash4k);
        assertEquals("Inconsistent hash from /authfs/6: ", expectedHash4k1, actualHash4k1);
    }

    @Test
    public void testReadWithFsverityVerification_TamperedMerkleTree() throws Exception {
        // Setup
        runFdServerOnAndroid(
                "--open-ro 3:input.4m --open-ro 4:input.4m.fsv_meta.bad_merkle", "--ro-fds 3:4");
        runAuthFsOnMicrodroid("--remote-ro-file 3:" + DIGEST_4M);

        // Verify
        assertThat(copyFile(sMicrodroid, MOUNT_DIR + "/3", "/dev/null")).isFailed();
    }

    @Test
    public void testReadWithFsverityVerification_FdServerUsesRealFsverityData() throws Exception {
        // Setup (fs-verity is enabled for input.file in AndroidTest.xml)
        runFdServerOnAndroid("--open-ro 3:input.file", "--ro-fds 3");
        String expectedDigest =
                sAndroid.run(FSVERITY_BIN + " digest --compact " + TEST_DIR + "/input.file");
        runAuthFsOnMicrodroid("--remote-ro-file 3:sha256-" + expectedDigest);

        // Action
        String actualHash = computeFileHash(sMicrodroid, MOUNT_DIR + "/3");

        // Verify
        String expectedHash = computeFileHash(sAndroid, TEST_DIR + "/input.file");
        assertEquals("Inconsistent hash from /authfs/3: ", expectedHash, actualHash);
    }

    @Test
    public void testWriteThroughCorrectly() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-rw 3:" + TEST_OUTPUT_DIR + "/out.file", "--rw-fds 3");
        runAuthFsOnMicrodroid("--remote-new-rw-file 3");

        // Action
        String srcPath = "/system/bin/linker64";
        String destPath = MOUNT_DIR + "/3";
        String backendPath = TEST_OUTPUT_DIR + "/out.file";
        assertThat(copyFile(sMicrodroid, srcPath, destPath)).isSuccess();

        // Verify
        String expectedHash = computeFileHash(sMicrodroid, srcPath);
        expectBackingFileConsistency(destPath, backendPath, expectedHash);
    }

    @Test
    public void testWriteFailedIfDetectsTampering() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-rw 3:" + TEST_OUTPUT_DIR + "/out.file", "--rw-fds 3");
        runAuthFsOnMicrodroid("--remote-new-rw-file 3");

        String srcPath = "/system/bin/linker64";
        String destPath = MOUNT_DIR + "/3";
        String backendPath = TEST_OUTPUT_DIR + "/out.file";
        assertThat(copyFile(sMicrodroid, srcPath, destPath)).isSuccess();

        // Action
        // Tampering with the first 2 4K-blocks of the backing file.
        assertThat(
                        writeZerosAtFileOffset(
                                sAndroid,
                                backendPath,
                                /* offset */ 0, /* number */
                                8192, /* writeThrough */
                                false))
                .isSuccess();

        // Verify
        // Write to a block partially requires a read back to calculate the new hash. It should fail
        // when the content is inconsistent to the known hash. Use direct I/O to avoid simply
        // writing to the filesystem cache.
        assertThat(
                        writeZerosAtFileOffset(
                                sMicrodroid,
                                destPath,
                                /* offset */ 0, /* number */
                                1024, /* writeThrough */
                                true))
                .isFailed();

        // A full 4K write does not require to read back, so write can succeed even if the backing
        // block has already been tampered.
        assertThat(
                        writeZerosAtFileOffset(
                                sMicrodroid,
                                destPath,
                                /* offset */ 4096, /* number */
                                4096, /* writeThrough */
                                false))
                .isSuccess();

        // Otherwise, a partial write with correct backing file should still succeed.
        assertThat(
                        writeZerosAtFileOffset(
                                sMicrodroid,
                                destPath,
                                /* offset */ 8192, /* number */
                                1024, /* writeThrough */
                                false))
                .isSuccess();
    }

    @Test
    public void testReadFailedIfDetectsTampering() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-rw 3:" + TEST_OUTPUT_DIR + "/out.file", "--rw-fds 3");
        runAuthFsOnMicrodroid("--remote-new-rw-file 3");

        String srcPath = "/system/bin/linker64";
        String destPath = MOUNT_DIR + "/3";
        String backendPath = TEST_OUTPUT_DIR + "/out.file";
        assertThat(copyFile(sMicrodroid, srcPath, destPath)).isSuccess();

        // Action
        // Tampering with the first 4K-block of the backing file.
        assertThat(
                        writeZerosAtFileOffset(
                                sAndroid,
                                backendPath,
                                /* offset */ 0, /* number */
                                4096, /* writeThrough */
                                false))
                .isSuccess();

        // Verify
        // Force dropping the page cache, so that the next read can be validated.
        sMicrodroid.run("echo 1 > /proc/sys/vm/drop_caches");
        // A read will fail if the backing data has been tampered.
        assertThat(checkReadAt(sMicrodroid, destPath, /* offset */ 0, /* number */ 4096))
                .isFailed();
        assertThat(checkReadAt(sMicrodroid, destPath, /* offset */ 4096, /* number */ 4096))
                .isSuccess();
    }

    @Test
    public void testResizeFailedIfDetectsTampering() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-rw 3:" + TEST_OUTPUT_DIR + "/out.file", "--rw-fds 3");
        runAuthFsOnMicrodroid("--remote-new-rw-file 3");

        String outputPath = MOUNT_DIR + "/3";
        String backendPath = TEST_OUTPUT_DIR + "/out.file";
        createFileWithOnes(sMicrodroid, outputPath, 8192);

        // Action
        // Tampering with the last 4K-block of the backing file.
        assertThat(
                        writeZerosAtFileOffset(
                                sAndroid,
                                backendPath,
                                /* offset */ 4096, /* number */
                                1, /* writeThrough */
                                false))
                .isSuccess();

        // Verify
        // A resize (to a non-multiple of 4K) will fail if the last backing chunk has been
        // tampered. The original data is necessary (and has to be verified) to calculate the new
        // hash with shorter data.
        assertThat(resizeFile(sMicrodroid, outputPath, 8000)).isFailed();
    }

    @Test
    public void testFileResize() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-rw 3:" + TEST_OUTPUT_DIR + "/out.file", "--rw-fds 3");
        runAuthFsOnMicrodroid("--remote-new-rw-file 3");
        String outputPath = MOUNT_DIR + "/3";
        String backendPath = TEST_OUTPUT_DIR + "/out.file";

        // Action & Verify
        createFileWithOnes(sMicrodroid, outputPath, 10000);
        assertEquals(getFileSizeInBytes(sMicrodroid, outputPath), 10000);
        expectBackingFileConsistency(
                outputPath,
                backendPath,
                "684ad25fdc2bbb80cbc910dd1bde6d5499ccf860ca6ee44704b77ec445271353");

        assertThat(resizeFile(sMicrodroid, outputPath, 15000)).isSuccess();
        assertEquals(getFileSizeInBytes(sMicrodroid, outputPath), 15000);
        expectBackingFileConsistency(
                outputPath,
                backendPath,
                "567c89f62586e0d33369157afdfe99a2fa36cdffb01e91dcdc0b7355262d610d");

        assertThat(resizeFile(sMicrodroid, outputPath, 5000)).isSuccess();
        assertEquals(getFileSizeInBytes(sMicrodroid, outputPath), 5000);
        expectBackingFileConsistency(
                outputPath,
                backendPath,
                "e53130831c13dabff71d5d1797e3aaa467b4b7d32b3b8782c4ff03d76976f2aa");
    }

    @Test
    public void testOutputDirectory_WriteNewFiles() throws Exception {
        // Setup
        String androidOutputDir = TEST_OUTPUT_DIR + "/dir";
        String authfsOutputDir = MOUNT_DIR + "/3";
        sAndroid.run("mkdir " + androidOutputDir);
        runFdServerOnAndroid("--open-dir 3:" + androidOutputDir, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        // Action & Verify
        // Can create a new file to write.
        String expectedAndroidPath = androidOutputDir + "/file";
        String authfsPath = authfsOutputDir + "/file";
        createFileWithOnes(sMicrodroid, authfsPath, 10000);
        assertEquals(getFileSizeInBytes(sMicrodroid, authfsPath), 10000);
        expectBackingFileConsistency(
                authfsPath,
                expectedAndroidPath,
                "684ad25fdc2bbb80cbc910dd1bde6d5499ccf860ca6ee44704b77ec445271353");

        // Regular file operations work, e.g. resize.
        assertThat(resizeFile(sMicrodroid, authfsPath, 15000)).isSuccess();
        assertEquals(getFileSizeInBytes(sMicrodroid, authfsPath), 15000);
        expectBackingFileConsistency(
                authfsPath,
                expectedAndroidPath,
                "567c89f62586e0d33369157afdfe99a2fa36cdffb01e91dcdc0b7355262d610d");
    }

    @Test
    public void testOutputDirectory_MkdirAndWriteFile() throws Exception {
        // Setup
        String androidOutputDir = TEST_OUTPUT_DIR + "/dir";
        String authfsOutputDir = MOUNT_DIR + "/3";
        sAndroid.run("mkdir " + androidOutputDir);
        runFdServerOnAndroid("--open-dir 3:" + androidOutputDir, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        // Action
        // Can create nested directories and can create a file in one.
        sMicrodroid.run("mkdir " + authfsOutputDir + "/new_dir");
        sMicrodroid.run("mkdir -p " + authfsOutputDir + "/we/need/to/go/deeper");
        createFileWithOnes(sMicrodroid, authfsOutputDir + "/new_dir/file1", 10000);
        createFileWithOnes(sMicrodroid, authfsOutputDir + "/we/need/file2", 10000);

        // Verify
        // Directories show up in Android.
        sAndroid.run("test -d " + androidOutputDir + "/new_dir");
        sAndroid.run("test -d " + androidOutputDir + "/we/need/to/go/deeper");
        // Files exist in Android. Hashes on Microdroid and Android are consistent.
        assertEquals(getFileSizeInBytes(sMicrodroid, authfsOutputDir + "/new_dir/file1"), 10000);
        expectBackingFileConsistency(
                authfsOutputDir + "/new_dir/file1",
                androidOutputDir + "/new_dir/file1",
                "684ad25fdc2bbb80cbc910dd1bde6d5499ccf860ca6ee44704b77ec445271353");
        // Same to file in a nested directory.
        assertEquals(getFileSizeInBytes(sMicrodroid, authfsOutputDir + "/we/need/file2"), 10000);
        expectBackingFileConsistency(
                authfsOutputDir + "/we/need/file2",
                androidOutputDir + "/we/need/file2",
                "684ad25fdc2bbb80cbc910dd1bde6d5499ccf860ca6ee44704b77ec445271353");
    }

    @Test
    public void testOutputDirectory_CreateAndTruncateExistingFile() throws Exception {
        // Setup
        String androidOutputDir = TEST_OUTPUT_DIR + "/dir";
        String authfsOutputDir = MOUNT_DIR + "/3";
        sAndroid.run("mkdir " + androidOutputDir);
        runFdServerOnAndroid("--open-dir 3:" + androidOutputDir, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        // Action & Verify
        sMicrodroid.run("echo -n foo > " + authfsOutputDir + "/file");
        assertEquals(getFileSizeInBytes(sMicrodroid, authfsOutputDir + "/file"), 3);
        // Can override a file and write normally.
        createFileWithOnes(sMicrodroid, authfsOutputDir + "/file", 10000);
        assertEquals(getFileSizeInBytes(sMicrodroid, authfsOutputDir + "/file"), 10000);
        expectBackingFileConsistency(
                authfsOutputDir + "/file",
                androidOutputDir + "/file",
                "684ad25fdc2bbb80cbc910dd1bde6d5499ccf860ca6ee44704b77ec445271353");
    }

    @Test
    public void testOutputDirectory_CanDeleteFile() throws Exception {
        // Setup
        String androidOutputDir = TEST_OUTPUT_DIR + "/dir";
        String authfsOutputDir = MOUNT_DIR + "/3";
        sAndroid.run("mkdir " + androidOutputDir);
        runFdServerOnAndroid("--open-dir 3:" + androidOutputDir, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        sMicrodroid.run("echo -n foo > " + authfsOutputDir + "/file");
        sMicrodroid.run("test -f " + authfsOutputDir + "/file");
        sAndroid.run("test -f " + androidOutputDir + "/file");

        // Action & Verify
        sMicrodroid.run("rm " + authfsOutputDir + "/file");
        sMicrodroid.run("test ! -f " + authfsOutputDir + "/file");
        sAndroid.run("test ! -f " + androidOutputDir + "/file");
    }

    @Test
    public void testOutputDirectory_CanDeleteDirectoryOnlyIfEmpty() throws Exception {
        // Setup
        String androidOutputDir = TEST_OUTPUT_DIR + "/dir";
        String authfsOutputDir = MOUNT_DIR + "/3";
        sAndroid.run("mkdir " + androidOutputDir);
        runFdServerOnAndroid("--open-dir 3:" + androidOutputDir, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        sMicrodroid.run("mkdir -p " + authfsOutputDir + "/dir/dir2");
        sMicrodroid.run("echo -n foo > " + authfsOutputDir + "/dir/file");
        sAndroid.run("test -d " + androidOutputDir + "/dir/dir2");

        // Action & Verify
        sMicrodroid.run("rmdir " + authfsOutputDir + "/dir/dir2");
        sMicrodroid.run("test ! -d " + authfsOutputDir + "/dir/dir2");
        sAndroid.run("test ! -d " + androidOutputDir + "/dir/dir2");
        // Can only delete a directory if empty
        assertThat(sMicrodroid.runForResult("rmdir " + authfsOutputDir + "/dir")).isFailed();
        sMicrodroid.run("test -d " + authfsOutputDir + "/dir"); // still there
        sMicrodroid.run("rm " + authfsOutputDir + "/dir/file");
        sMicrodroid.run("rmdir " + authfsOutputDir + "/dir");
        sMicrodroid.run("test ! -d " + authfsOutputDir + "/dir");
        sAndroid.run("test ! -d " + androidOutputDir + "/dir");
    }

    @Test
    public void testOutputDirectory_CannotRecreateDirectoryIfNameExists() throws Exception {
        // Setup
        String androidOutputDir = TEST_OUTPUT_DIR + "/dir";
        String authfsOutputDir = MOUNT_DIR + "/3";
        sAndroid.run("mkdir " + androidOutputDir);
        runFdServerOnAndroid("--open-dir 3:" + androidOutputDir, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        sMicrodroid.run("touch " + authfsOutputDir + "/some_file");
        sMicrodroid.run("mkdir " + authfsOutputDir + "/some_dir");
        sMicrodroid.run("touch " + authfsOutputDir + "/some_dir/file");
        sMicrodroid.run("mkdir " + authfsOutputDir + "/some_dir/dir");

        // Action & Verify
        // Cannot create directory if an entry with the same name already exists.
        assertThat(sMicrodroid.runForResult("mkdir " + authfsOutputDir + "/some_file")).isFailed();
        assertThat(sMicrodroid.runForResult("mkdir " + authfsOutputDir + "/some_dir")).isFailed();
        assertThat(sMicrodroid.runForResult("mkdir " + authfsOutputDir + "/some_dir/file"))
                .isFailed();
        assertThat(sMicrodroid.runForResult("mkdir " + authfsOutputDir + "/some_dir/dir"))
                .isFailed();
    }

    @Test
    public void testOutputDirectory_WriteToFdOfDeletedFile() throws Exception {
        // Setup
        String authfsOutputDir = MOUNT_DIR + "/3";
        String androidOutputDir = TEST_OUTPUT_DIR + "/dir";
        sAndroid.run("mkdir " + androidOutputDir);
        runFdServerOnAndroid("--open-dir 3:" + androidOutputDir, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        // Create a file with some data. Test the existence.
        String outputPath = authfsOutputDir + "/out";
        String androidOutputPath = androidOutputDir + "/out";
        sMicrodroid.run("echo -n 123 > " + outputPath);
        sMicrodroid.run("test -f " + outputPath);
        sAndroid.run("test -f " + androidOutputPath);

        // Action
        String output =
                sMicrodroid.run(
                        // Open the file for append and read
                        "exec 4>>"
                                + outputPath
                                + " 5<"
                                + outputPath
                                + "; "
                                // Delete the file from the directory
                                + "rm "
                                + outputPath
                                + "; "
                                // Append more data to the file descriptor
                                + "echo -n 456 >&4; "
                                // Print the whole file from the file descriptor
                                + "cat <&5");

        // Verify
        // Output contains all written data, while the files are deleted.
        assertEquals("123456", output);
        sMicrodroid.run("test ! -f " + outputPath);
        sAndroid.run("test ! -f " + androidOutputDir + "/out");
    }

    @Test
    public void testInputDirectory_CanReadFile() throws Exception {
        // Setup
        String authfsInputDir = MOUNT_DIR + "/3";
        runFdServerOnAndroid("--open-dir 3:" + TEST_DIR, "--ro-dirs 3");
        runAuthFsOnMicrodroid("--remote-ro-dir 3:" + INPUT_MANIFEST_PATH + ":");

        // Action
        String actualHash = computeFileHash(sMicrodroid, authfsInputDir + "/input.4m");

        // Verify
        String expectedHash = computeFileHash(sAndroid, TEST_DIR + "/input.4m");
        assertEquals("Expect consistent hash through /authfs/3: ", expectedHash, actualHash);
    }

    @Test
    public void testInputDirectory_OnlyAllowlistedFilesExist() throws Exception {
        // Setup
        String authfsInputDir = MOUNT_DIR + "/3";
        runFdServerOnAndroid("--open-dir 3:" + TEST_DIR, "--ro-dirs 3");
        runAuthFsOnMicrodroid("--remote-ro-dir 3:" + INPUT_MANIFEST_PATH + ":");

        // Verify
        sMicrodroid.run("test -f " + authfsInputDir + "/input.4k");
        assertThat(sMicrodroid.runForResult("test -f " + authfsInputDir + "/input.4k.fsv_meta"))
                .isFailed();
    }

    @Test
    public void testReadOutputDirectory() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-dir 3:" + TEST_OUTPUT_DIR, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        // Action
        String authfsOutputDir = MOUNT_DIR + "/3";
        sMicrodroid.run("mkdir -p " + authfsOutputDir + "/dir/dir2/dir3");
        sMicrodroid.run("touch " + authfsOutputDir + "/dir/dir2/dir3/file1");
        sMicrodroid.run("touch " + authfsOutputDir + "/dir/dir2/dir3/file2");
        sMicrodroid.run("touch " + authfsOutputDir + "/dir/dir2/dir3/file3");
        sMicrodroid.run("touch " + authfsOutputDir + "/file");

        // Verify
        String[] actual = sMicrodroid.run("cd " + authfsOutputDir + "; find |sort").split("\n");
        String[] expected =
                new String[] {
                    ".",
                    "./dir",
                    "./dir/dir2",
                    "./dir/dir2/dir3",
                    "./dir/dir2/dir3/file1",
                    "./dir/dir2/dir3/file2",
                    "./dir/dir2/dir3/file3",
                    "./file"
                };
        assertEquals(expected, actual);

        // Add more entries.
        sMicrodroid.run("mkdir -p " + authfsOutputDir + "/dir2");
        sMicrodroid.run("touch " + authfsOutputDir + "/file2");
        // Check new entries. Also check that the types are correct.
        actual =
                sMicrodroid
                        .run("cd " + authfsOutputDir + "; find -maxdepth 1 -type f |sort")
                        .split("\n");
        expected = new String[] {"./file", "./file2"};
        assertEquals(expected, actual);
        actual =
                sMicrodroid
                        .run("cd " + authfsOutputDir + "; find -maxdepth 1 -type d |sort")
                        .split("\n");
        expected = new String[] {".", "./dir", "./dir2"};
        assertEquals(expected, actual);
    }

    @Test
    public void testChmod_File() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-rw 3:" + TEST_OUTPUT_DIR + "/file", "--rw-fds 3");
        runAuthFsOnMicrodroid("--remote-new-rw-file 3");

        // Action & Verify
        // Change mode
        sMicrodroid.run("chmod 321 " + MOUNT_DIR + "/3");
        expectFileMode("--wx-w---x", MOUNT_DIR + "/3", TEST_OUTPUT_DIR + "/file");
        // Can't set the disallowed bits
        assertThat(sMicrodroid.runForResult("chmod +s " + MOUNT_DIR + "/3")).isFailed();
        assertThat(sMicrodroid.runForResult("chmod +t " + MOUNT_DIR + "/3")).isFailed();
    }

    @Test
    public void testChmod_Dir() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-dir 3:" + TEST_OUTPUT_DIR, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        // Action & Verify
        String authfsOutputDir = MOUNT_DIR + "/3";
        // Create with umask
        sMicrodroid.run("umask 000; mkdir " + authfsOutputDir + "/dir");
        sMicrodroid.run("umask 022; mkdir " + authfsOutputDir + "/dir/dir2");
        expectFileMode("drwxrwxrwx", authfsOutputDir + "/dir", TEST_OUTPUT_DIR + "/dir");
        expectFileMode("drwxr-xr-x", authfsOutputDir + "/dir/dir2", TEST_OUTPUT_DIR + "/dir/dir2");
        // Change mode
        sMicrodroid.run("chmod -w " + authfsOutputDir + "/dir/dir2");
        expectFileMode("dr-xr-xr-x", authfsOutputDir + "/dir/dir2", TEST_OUTPUT_DIR + "/dir/dir2");
        sMicrodroid.run("chmod 321 " + authfsOutputDir + "/dir");
        expectFileMode("d-wx-w---x", authfsOutputDir + "/dir", TEST_OUTPUT_DIR + "/dir");
        // Can't set the disallowed bits
        assertThat(sMicrodroid.runForResult("chmod +s " + authfsOutputDir + "/dir/dir2"))
                .isFailed();
        assertThat(sMicrodroid.runForResult("chmod +t " + authfsOutputDir + "/dir")).isFailed();
    }

    @Test
    public void testChmod_FileInOutputDirectory() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-dir 3:" + TEST_OUTPUT_DIR, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        // Action & Verify
        String authfsOutputDir = MOUNT_DIR + "/3";
        // Create with umask
        sMicrodroid.run("umask 000; echo -n foo > " + authfsOutputDir + "/file");
        sMicrodroid.run("umask 022; echo -n foo > " + authfsOutputDir + "/file2");
        expectFileMode("-rw-rw-rw-", authfsOutputDir + "/file", TEST_OUTPUT_DIR + "/file");
        expectFileMode("-rw-r--r--", authfsOutputDir + "/file2", TEST_OUTPUT_DIR + "/file2");
        // Change mode
        sMicrodroid.run("chmod -w " + authfsOutputDir + "/file");
        expectFileMode("-r--r--r--", authfsOutputDir + "/file", TEST_OUTPUT_DIR + "/file");
        sMicrodroid.run("chmod 321 " + authfsOutputDir + "/file2");
        expectFileMode("--wx-w---x", authfsOutputDir + "/file2", TEST_OUTPUT_DIR + "/file2");
        // Can't set the disallowed bits
        assertThat(sMicrodroid.runForResult("chmod +s " + authfsOutputDir + "/file")).isFailed();
        assertThat(sMicrodroid.runForResult("chmod +t " + authfsOutputDir + "/file2")).isFailed();
    }

    @Test
    public void testStatfs() throws Exception {
        // Setup
        runFdServerOnAndroid("--open-dir 3:" + TEST_OUTPUT_DIR, "--rw-dirs 3");
        runAuthFsOnMicrodroid("--remote-new-rw-dir 3");

        // Verify
        // Magic matches. Has only 2 inodes (root and "/3").
        assertEquals(
                mAuthFsTestRule.FUSE_SUPER_MAGIC_HEX + " 2",
                sMicrodroid.run("stat -f -c '%t %c' " + MOUNT_DIR));
    }

    private void expectBackingFileConsistency(
            String authFsPath, String backendPath, String expectedHash)
            throws DeviceNotAvailableException {
        String hashOnAuthFs = computeFileHash(sMicrodroid, authFsPath);
        assertEquals("File hash is different to expectation", expectedHash, hashOnAuthFs);

        String hashOfBackingFile = computeFileHash(sAndroid, backendPath);
        assertEquals(
                "Inconsistent file hash on the backend storage", hashOnAuthFs, hashOfBackingFile);
    }

    private static String computeFileHash(CommandRunner runner, String path)
            throws DeviceNotAvailableException {
        String result = runner.run("sha256sum " + path);
        String[] tokens = result.split("\\s");
        if (tokens.length > 0) {
            return tokens[0];
        } else {
            CLog.e("Unrecognized output by sha256sum: " + result);
            return "";
        }
    }

    private static CommandResult copyFile(CommandRunner runner, String src, String dest)
            throws DeviceNotAvailableException {
        // toybox's cp(1) implementation ignores most read(2) errors, and it's unclear what the
        // canonical behavior should be (not mentioned in manpage). For this test, use cat(1) in
        // order to fail on I/O error.
        return runner.runForResult("cat " + src + " > " + dest);
    }

    private void expectFileMode(String expected, String microdroidPath, String androidPath)
            throws DeviceNotAvailableException {
        String actual = sMicrodroid.run("stat -c '%A' " + microdroidPath);
        assertEquals("Inconsistent mode for " + microdroidPath, expected, actual);

        actual = sAndroid.run("stat -c '%A' " + androidPath);
        assertEquals("Inconsistent mode for " + androidPath + " (android)", expected, actual);
    }

    private static CommandResult resizeFile(CommandRunner runner, String path, long size)
            throws DeviceNotAvailableException {
        return runner.runForResult("truncate -c -s " + size + " " + path);
    }

    private static long getFileSizeInBytes(CommandRunner runner, String path)
            throws DeviceNotAvailableException {
        return Long.parseLong(runner.run("stat -c '%s' " + path));
    }

    private static void createFileWithOnes(CommandRunner runner, String filePath, long numberOfOnes)
            throws DeviceNotAvailableException {
        runner.run(
                "yes $'\\x01' | tr -d '\\n' | dd bs=1 count=" + numberOfOnes + " of=" + filePath);
    }

    private static CommandResult checkReadAt(
            CommandRunner runner, String filePath, long offset, long size)
            throws DeviceNotAvailableException {
        String cmd = "dd if=" + filePath + " of=/dev/null bs=1 count=" + size;
        if (offset > 0) {
            cmd += " skip=" + offset;
        }
        return runner.runForResult(cmd);
    }

    private CommandResult writeZerosAtFileOffset(
            CommandRunner runner,
            String filePath,
            long offset,
            long numberOfZeros,
            boolean writeThrough)
            throws DeviceNotAvailableException {
        String cmd =
                "dd if=/dev/zero of=" + filePath + " bs=1 count=" + numberOfZeros + " conv=notrunc";
        if (offset > 0) {
            cmd += " seek=" + offset;
        }
        if (writeThrough) {
            cmd += " direct";
        }
        return runner.runForResult(cmd);
    }

    private void runAuthFsOnMicrodroid(String flags) {
        mAuthFsTestRule.runAuthFsOnMicrodroid(flags);
    }

    private void runFdServerOnAndroid(String helperFlags, String fdServerFlags)
            throws DeviceNotAvailableException {
        mAuthFsTestRule.runFdServerOnAndroid(helperFlags, fdServerFlags);
    }
}
