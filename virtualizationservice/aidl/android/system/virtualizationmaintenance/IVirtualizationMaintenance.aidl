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

import android.system.virtualizationmaintenance.IVirtualizationReconciliationCallback;

interface IVirtualizationMaintenance {
    /**
     * Notification that an app has been permanently removed, to allow related global state to
     * be removed.
     *
     * @param userId The Android user ID for whom the notification applies.
     */
    void appRemoved(int userId, int appId);

    /**
     * Notification that a user has been removed, to allow related global state to be removed.
     *
     * @param userId The Android user ID of the user.
     */
    void userRemoved(int userId);

    /*
     * Requests virtualization service to perform reconciliation of Secretkeeper secrets.
     * Secrets belonging to apps or users that no longer exist should be deleted.
     * The supplied callback allows for querying of existence.
     * This method should return on successful completion of the reconciliation process.
     * It should throw an exception if there is any failure, or if any of the callback
     * functions return {@code ERROR_STOP_REQUESTED}.
     */
    void performReconciliation(IVirtualizationReconciliationCallback callback);
}
