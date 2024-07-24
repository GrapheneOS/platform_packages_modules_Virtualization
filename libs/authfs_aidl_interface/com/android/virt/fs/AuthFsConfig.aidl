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

/** @hide */
parcelable AuthFsConfig {
    parcelable InputFdAnnotation {
        /**
         * File descriptor number to be passed to the program.  This is also the same file
         * descriptor number used in the backend server.
         */
        int fd;
    }

    parcelable OutputFdAnnotation {
        /**
         * File descriptor number to be passed to the program.  This is also the same file
         * descriptor number used in the backend server.
         */
        int fd;
    }

    parcelable InputDirFdAnnotation {
        /**
         * File descriptor number to be passed to the program.  This is also the same file
         * descriptor number used in the backend server.
         */
        int fd;

        /**
         * A manifest file that includes serialized protobuf of
         * android.security.fsverity.FSVerityDigests. The path must be accessible to the
         * IAuthFsService.
         */
        String manifestPath;

        /**
         * Prefix path that should be stripped from the path in the manifest.
         */
        String prefix;
    }

    parcelable OutputDirFdAnnotation {
        /**
         * File descriptor number to be passed to the program.  This is also the same file
         * descriptor number used in the backend server.
         */
        int fd;
    }

    /** Port of the filesystem backend. */
    int port;

    /** Annotation for the remote input file descriptors. */
    InputFdAnnotation[] inputFdAnnotations;

    /** Annotation for the remote output file descriptors. */
    OutputFdAnnotation[] outputFdAnnotations;

    /** Annotation for the remote input directory descriptors. */
    InputDirFdAnnotation[] inputDirFdAnnotations;

    /** Annotation for the remote output directory descriptors. */
    OutputDirFdAnnotation[] outputDirFdAnnotations;
}
