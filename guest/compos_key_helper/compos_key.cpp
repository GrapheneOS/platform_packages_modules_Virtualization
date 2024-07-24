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

#include "compos_key.h"

#include <openssl/digest.h>
#include <openssl/hkdf.h>
#include <openssl/mem.h>

using android::base::ErrnoError;
using android::base::Error;
using android::base::Result;
using compos_key::Ed25519KeyPair;
using compos_key::Seed;
using compos_key::Signature;

namespace compos_key {
Result<Ed25519KeyPair> keyFromSeed(const Seed& seed) {
    Ed25519KeyPair result;
    ED25519_keypair_from_seed(result.public_key.data(), result.private_key.data(), seed.data());
    return result;
}

Result<Signature> sign(const PrivateKey& private_key, const uint8_t* data, size_t data_size) {
    Signature result;
    if (!ED25519_sign(result.data(), data, data_size, private_key.data())) {
        return Error() << "Failed to sign";
    }
    return result;
}

bool verify(const PublicKey& public_key, const Signature& signature, const uint8_t* data,
            size_t data_size) {
    return ED25519_verify(data, data_size, signature.data(), public_key.data()) == 1;
}
} // namespace compos_key
