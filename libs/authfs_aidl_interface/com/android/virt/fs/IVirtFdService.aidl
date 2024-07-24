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

/**
 * A service that works like a file server, where the files and directories are identified by
 * "remote FD" that may be pre-exchanged or created on request.
 *
 * When a binder error is returned and it is a service specific error, the error code is an errno
 * value which is an int.
 *
 * {@hide}
 */
interface IVirtFdService {
    /** Maximum content size that the service allows the client to request. */
    const int MAX_REQUESTING_DATA = 16384;

    /**
     * Returns the content of the given remote FD, from the offset, for the amount of requested size
     * or until EOF.
     */
    byte[] readFile(int fd, long offset, int size);

    /**
     * Returns the content of fs-verity compatible Merkle tree of the given remote FD, from the
     * offset, for the amount of requested size or until EOF.
     */
    byte[] readFsverityMerkleTree(int fd, long offset, int size);

    /** Returns the fs-verity signature of the given remote FD. */
    byte[] readFsveritySignature(int fd);

    /**
     * Writes the buffer to the given remote FD from the file's offset. Returns the number of bytes
     * written.
     */
    int writeFile(int fd, in byte[] buf, long offset);

    /** Resizes the file backed by the given remote FD to the new size. */
    void resize(int fd, long size);

    /** Returns the file size. */
    long getFileSize(int fd);

    /**
     * Opens a file given the remote directory FD.
     *
     * @param pathname The file path to open. Must be a related path.
     * @return file A remote FD that represents the opened file.
     */
    int openFileInDirectory(int dirFd, String pathname);

    /**
     * Creates a file given the remote directory FD.
     *
     * @param basename The file name to create. Must not contain directory separator.
     * @param mode File mode of the new file. See open(2).
     * @return file A remote FD that represents the new created file.
     */
    int createFileInDirectory(int dirFd, String basename, int mode);

    /**
     * Creates a directory inside the given remote directory FD.
     *
     * @param basename The directory name to create. Must not contain directory separator.
     * @param mode File mode of the new directory. See mkdir(2).
     * @return file FD that represents the new created directory.
     */
    int createDirectoryInDirectory(int dirFd, String basename, int mode);

    /**
     * Deletes a file in the given directory.
     *
     * @param basename The file name to delete. Must not contain directory separator.
     */
    void deleteFile(int dirFd, String basename);

    /**
     * Deletes a sub-directory in the given directory.
     *
     * @param basename The directory name to delete. Must not contain directory separator.
     */
    void deleteDirectory(int dirFd, String basename);

    /**
     * Changes mode of the FD.
     *
     * @param fd The FD to change.
     * @param mode New file mode to pass to chmod(2)/fchmod(2).
     */
    void chmod(int fd, int mode);

    /** Filesystem stats that AuthFS is interested in.*/
    parcelable FsStat {
        /** Block size of the filesystem */
        long blockSize;
        /** Fragment size of the filesystem */
        long fragmentSize;
        /** Number of blocks in the filesystem */
        long blockNumbers;
        /** Number of free blocks */
        long blockAvailable;
        /** Number of free inodes */
        long inodesAvailable;
        /** Maximum filename length */
        long maxFilename;
    }

    /** Returns relevant filesystem stats. */
    FsStat statfs();
}
