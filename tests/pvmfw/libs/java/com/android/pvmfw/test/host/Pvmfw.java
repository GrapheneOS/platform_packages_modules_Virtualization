/*
 * Copyright 2023 The Android Open Source Project
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

package com.android.pvmfw.test.host;

import static com.google.common.truth.Truth.assertThat;

import static java.nio.ByteOrder.LITTLE_ENDIAN;

import androidx.annotation.NonNull;
import androidx.annotation.Nullable;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.Objects;

/** pvmfw.bin with custom config payloads on host. */
public final class Pvmfw {
    private static final int SIZE_8B = 8; // 8 bytes
    private static final int SIZE_4K = 4 << 10; // 4 KiB, PAGE_SIZE
    private static final int BUFFER_SIZE = 1024;
    private static final int HEADER_MAGIC = 0x666d7670;
    private static final int HEADER_DEFAULT_VERSION = makeVersion(1, 2);
    private static final int HEADER_FLAGS = 0;

    private static final int PVMFW_ENTRY_BCC = 0;
    private static final int PVMFW_ENTRY_DP = 1;
    private static final int PVMFW_ENTRY_VM_DTBO = 2;
    private static final int PVMFW_ENTRY_VM_REFERENCE_DT = 3;
    private static final int PVMFW_ENTRY_MAX = 4;

    @NonNull private final File mPvmfwBinFile;
    private final File[] mEntries;
    private final int mEntryCnt;
    private final int mVersion;

    public static int makeVersion(int major, int minor) {
        return ((major & 0xFFFF) << 16) | (minor & 0xFFFF);
    }

    private Pvmfw(
            @NonNull File pvmfwBinFile,
            @NonNull File bccFile,
            @Nullable File debugPolicyFile,
            @Nullable File vmDtboFile,
            @Nullable File vmReferenceDtFile,
            int version) {
        mPvmfwBinFile = Objects.requireNonNull(pvmfwBinFile);

        if (version >= makeVersion(1, 2)) {
            mEntryCnt = PVMFW_ENTRY_VM_REFERENCE_DT + 1;
        } else if (version >= makeVersion(1, 1)) {
            mEntryCnt = PVMFW_ENTRY_VM_DTBO + 1;
        } else {
            mEntryCnt = PVMFW_ENTRY_DP + 1;
        }

        mEntries = new File[PVMFW_ENTRY_MAX];
        mEntries[PVMFW_ENTRY_BCC] = Objects.requireNonNull(bccFile);
        mEntries[PVMFW_ENTRY_DP] = debugPolicyFile;

        if (PVMFW_ENTRY_VM_DTBO < mEntryCnt) {
            mEntries[PVMFW_ENTRY_VM_DTBO] = vmDtboFile;
        }
        if (PVMFW_ENTRY_VM_REFERENCE_DT < mEntryCnt) {
            mEntries[PVMFW_ENTRY_VM_REFERENCE_DT] = Objects.requireNonNull(vmReferenceDtFile);
        }

        mVersion = version;
    }

    /**
     * Serializes pvmfw.bin and its config, as written in the <a
     * href="https://android.googlesource.com/platform/packages/modules/Virtualization/+/master/pvmfw/README.md">README.md</a>
     */
    public void serialize(@NonNull File outFile) throws IOException {
        Objects.requireNonNull(outFile);

        int headerSize = alignTo(getHeaderSize(), SIZE_8B);
        int[] entryOffsets = new int[mEntryCnt];
        int[] entrySizes = new int[mEntryCnt];

        entryOffsets[PVMFW_ENTRY_BCC] = headerSize;
        entrySizes[PVMFW_ENTRY_BCC] = (int) mEntries[PVMFW_ENTRY_BCC].length();

        for (int i = 1; i < mEntryCnt; i++) {
            entryOffsets[i] = alignTo(entryOffsets[i - 1] + entrySizes[i - 1], SIZE_8B);
            entrySizes[i] = mEntries[i] == null ? 0 : (int) mEntries[i].length();
        }

        int totalSize = alignTo(entryOffsets[mEntryCnt - 1] + entrySizes[mEntryCnt - 1], SIZE_8B);

        ByteBuffer header = ByteBuffer.allocate(headerSize).order(LITTLE_ENDIAN);
        header.putInt(HEADER_MAGIC);
        header.putInt(mVersion);
        header.putInt(totalSize);
        header.putInt(HEADER_FLAGS);
        for (int i = 0; i < mEntryCnt; i++) {
            header.putInt(entryOffsets[i]);
            header.putInt(entrySizes[i]);
        }

        try (FileOutputStream pvmfw = new FileOutputStream(outFile)) {
            appendFile(pvmfw, mPvmfwBinFile);
            padTo(pvmfw, SIZE_4K);

            int baseOffset = (int) pvmfw.getChannel().size();
            pvmfw.write(header.array());

            for (int i = 0; i < mEntryCnt; i++) {
                padTo(pvmfw, SIZE_8B);
                if (mEntries[i] != null) {
                    assertThat((int) pvmfw.getChannel().size() - baseOffset)
                            .isEqualTo(entryOffsets[i]);
                    appendFile(pvmfw, mEntries[i]);
                }
            }

            padTo(pvmfw, SIZE_4K);
        }
    }

    private void appendFile(@NonNull FileOutputStream out, @NonNull File inFile)
            throws IOException {
        try (FileInputStream in = new FileInputStream(inFile)) {
            in.transferTo(out);
        }
    }

    private void padTo(@NonNull FileOutputStream out, int size) throws IOException {
        int streamSize = (int) out.getChannel().size();
        for (int i = streamSize; i < alignTo(streamSize, size); i++) {
            out.write(0); // write byte.
        }
    }

    private int getHeaderSize() {
        // Header + (entry offset, entry, size) * mEntryCnt
        return Integer.BYTES * (4 + mEntryCnt * 2);
    }

    private static int alignTo(int x, int size) {
        return (x + size - 1) & ~(size - 1);
    }

    private static int getMajorVersion(int version) {
        return (version >> 16) & 0xFFFF;
    }

    private static int getMinorVersion(int version) {
        return version & 0xFFFF;
    }

    /** Builder for {@link Pvmfw}. */
    public static final class Builder {
        @NonNull private final File mPvmfwBinFile;
        @NonNull private final File mBccFile;
        @Nullable private File mDebugPolicyFile;
        @Nullable private File mVmDtboFile;
        @Nullable private File mVmReferenceDtFile;
        private int mVersion;

        public Builder(@NonNull File pvmfwBinFile, @NonNull File bccFile) {
            mPvmfwBinFile = Objects.requireNonNull(pvmfwBinFile);
            mBccFile = Objects.requireNonNull(bccFile);
            mVersion = HEADER_DEFAULT_VERSION;
        }

        @NonNull
        public Builder setDebugPolicyOverlay(@Nullable File debugPolicyFile) {
            mDebugPolicyFile = debugPolicyFile;
            return this;
        }

        @NonNull
        public Builder setVmDtbo(@Nullable File vmDtboFile) {
            mVmDtboFile = vmDtboFile;
            return this;
        }

        @NonNull
        public Builder setVmReferenceDt(@Nullable File vmReferenceDtFile) {
            mVmReferenceDtFile = vmReferenceDtFile;
            return this;
        }

        @NonNull
        public Builder setVersion(int major, int minor) {
            mVersion = makeVersion(major, minor);
            return this;
        }

        @NonNull
        public Pvmfw build() {
            return new Pvmfw(
                    mPvmfwBinFile,
                    mBccFile,
                    mDebugPolicyFile,
                    mVmDtboFile,
                    mVmReferenceDtFile,
                    mVersion);
        }
    }
}
