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

package android.system.virtualizationmaintenance;

/*
 * Callback interface provided when reconciliation is performed to allow verifying whether users
 * and apps currently exist.
 */
interface IVirtualizationReconciliationCallback {
    /*
     * Service-specific error code indicating that the job scheduler has requested that we
     * stop
     */
    const int ERROR_STOP_REQUESTED = 1;

    /*
     * Determine whether users with selected IDs currently exist. The result is an array of booleans
     * which indicate whether the corresponding entry in the {@code userIds} array is a valid
     * user ID.
     */
    boolean[] doUsersExist(in int[] userIds);

    /*
     * Determine whether apps with selected app IDs currently exist for a specific user.
     * The result is an array of booleans which indicate whether the corresponding entry in the
     * {@code appIds} array is a current app ID for the user.
     */
    boolean[] doAppsExist(int userId, in int[] appIds);
}
