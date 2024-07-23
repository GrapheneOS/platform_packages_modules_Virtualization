/*
 * Copyright 2021 The Android Open Source Project
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
package android.system.virtualizationservice;

import android.system.virtualizationservice.Partition;

/** A disk image to be made available to the VM. */
parcelable DiskImage {
    /**
     * The disk image, if it already exists. Exactly one of this and `partitions` must be specified.
     */
    @nullable ParcelFileDescriptor image;

    /** Whether this disk should be writable by the VM. */
    boolean writable;

    /** Partition images to be assembled into a composite image. */
    Partition[] partitions;
}
