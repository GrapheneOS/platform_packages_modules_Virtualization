/*
 * Copyright 2022 The Android Open Source Project
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
package android.system.virtualizationcommon;

/**
 * Errors reported from within a VM.
 */
@Backing(type="int")
enum ErrorCode {
    /**
     * Error code for all other errors not listed below.
     */
    UNKNOWN = 0,

    /**
     * Error code indicating that the payload can't be verified due to various reasons (e.g invalid
     * merkle tree, invalid formats, etc).
     */
    PAYLOAD_VERIFICATION_FAILED = 1,

    /**
     * Error code indicating that the payload is verified, but has changed since the last boot.
     */
    PAYLOAD_CHANGED = 2,

    /**
     * Error code indicating that the payload config is invalid.
     */
    PAYLOAD_INVALID_CONFIG = 3,
}
