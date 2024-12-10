/*
 * Copyright 2024 The Android Open Source Project
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

/** Shared directory path between host and guest */
parcelable SharedPath {
    /** Shared path between host and guest */
    String sharedPath;

    /** UID of the path on the host */
    int hostUid;

    /** GID of the path on the host */
    int hostGid;

    /** UID of the path on the guest */
    int guestUid;

    /** GID of the path on the guest */
    int guestGid;

    /** umask settings for the path */
    int mask;

    /** virtiofs unique tag per path */
    String tag;

    /** socket name for vhost-user-fs */
    String socketPath;

    /** socket fd for crosvm to connect */
    @nullable ParcelFileDescriptor socketFd;

    /** crosvm started from appDomain */
    boolean appDomain;
}
