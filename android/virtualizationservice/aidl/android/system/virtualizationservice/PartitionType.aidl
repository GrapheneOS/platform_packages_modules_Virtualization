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

/**
 * Type of the writable partition that virtualizationservice creates via
 * initializeWritablePartition.
 */
@Backing(type="int")
enum PartitionType {
    /**
     * The partition is simply initialized as all zeros
     */
    RAW = 0,
    /**
     * The partition is initialized as an instance image which is formatted to hold per-VM secrets
     */
    ANDROID_VM_INSTANCE = 1,
    /**
     * The partition is initialized to back encryptedstore disk image formatted to indicate intent
     */
    ENCRYPTEDSTORE = 2,
}
