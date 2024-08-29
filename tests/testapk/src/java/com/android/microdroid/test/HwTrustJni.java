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

package com.android.microdroid.test;

class HwTrustJni {
    static {
        System.loadLibrary("hwtrust_jni");
    }

    /**
     * Validates a DICE chain.
     *
     * @param diceChain The dice chain to validate.
     * @param allowAnyMode Allow the chain's certificates to have any mode.
     * @return true if the dice chain is valid, false otherwise.
     */
    public static native boolean validateDiceChain(byte[] diceChain, boolean allowAnyMode);
}
