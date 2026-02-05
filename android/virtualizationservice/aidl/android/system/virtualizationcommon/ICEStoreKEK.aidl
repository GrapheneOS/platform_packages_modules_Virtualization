/*
 * Copyright 2026 The Android Open Source Project
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

/** @hide */
interface ICEStoreKEK {
    /**
     * Returns a KEK used to set up the ce store, or {@code null} if ce store
     * hasn't been set up yet.
     *
     * <p>If {@code null} is returned, then {@code microdroid_manager} should:
     *  1. Use the obtained salt to create a new key to set up ce store with.
     *  2. Encrypt it with a different key.
     *  3. Send resulting KEK back to the Android host by calling {@code onKEKCreated} callback.
     */
    @nullable byte[] getKEK();

    /**
     * A callback {@code microdroid_manager} should call when new KEK is created.
     *
     * <p>Android host is expected to store the resulting kek on disk (e.g. in app's private CE
     * directory). On subsequent VM boots {@code microdroid_manager} will request the KEK from
     * the Android host, and then use it to set up the ce store.
     */
    void onKEKCreated(in byte[] kek);
}
