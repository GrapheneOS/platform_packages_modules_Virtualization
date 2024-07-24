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

#pragma once

#include <android-base/result.h>
#include <openssl/curve25519.h>

#include <array>

namespace compos_key {
constexpr size_t ED25519_SEED_LEN = 32;

using PrivateKey = std::array<uint8_t, ED25519_PRIVATE_KEY_LEN>;
using PublicKey = std::array<uint8_t, ED25519_PUBLIC_KEY_LEN>;
using Seed = std::array<uint8_t, ED25519_SEED_LEN>;
using Signature = std::array<uint8_t, ED25519_SIGNATURE_LEN>;

struct Ed25519KeyPair {
    PrivateKey private_key;
    PublicKey public_key;
};

android::base::Result<Ed25519KeyPair> keyFromSeed(const Seed& seed);

android::base::Result<Signature> sign(const PrivateKey& private_key, const uint8_t* data,
                                      size_t data_size);

bool verify(const PublicKey& public_key, const Signature& signature, const uint8_t* data,
            size_t data_size);
} // namespace compos_key
