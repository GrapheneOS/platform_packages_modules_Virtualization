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

#include "verify_native.h"

#include <compos_key.h>

using rust::Slice;

bool verify(Slice<const uint8_t> public_key, Slice<const uint8_t> signature,
            Slice<const uint8_t> data) {
    compos_key::PublicKey public_key_array;
    compos_key::Signature signature_array;

    if (public_key.size() != public_key_array.size() ||
        signature.size() != signature_array.size()) {
        return false;
    }

    std::copy(public_key.begin(), public_key.end(), public_key_array.begin());
    std::copy(signature.begin(), signature.end(), signature_array.begin());

    return compos_key::verify(public_key_array, signature_array, data.data(), data.size());
}
