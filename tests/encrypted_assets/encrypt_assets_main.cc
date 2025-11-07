/*
 * Copyright 2025, The Android Open Source Project
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

#include <android-base/file.h>
#include <android-base/parseint.h>
#include <android-base/result.h>
#include <openssl/evp.h>

#include <bit>
#include <cstdint>
#include <iostream>
#include <span>
#include <string>
#include <vector>

namespace {

using android::base::ErrnoError;
using android::base::Error;
using android::base::Result;

constexpr size_t kAesBlockSize = 16;
constexpr size_t kDefaultSectorSize = 512;

Result<std::vector<uint8_t>> readBinaryFile(const std::string& path) {
    std::string str;
    if (!android::base::ReadFileToString(path, &str)) {
        return ErrnoError() << "Failed to read from " << path;
    }
    return std::vector<uint8_t>(str.begin(), str.end());
}

Result<void> writeBinaryFile(const std::vector<uint8_t>& data, const std::string& path) {
    std::string str(data.begin(), data.end());
    if (!android::base::WriteStringToFile(str, path)) {
        return ErrnoError() << "Failed to write to " << path;
    }
    return {};
}

struct Args {
    std::vector<uint8_t> key;
    std::vector<uint8_t> input;
    std::string output_path;
    size_t sector_size;
};

Result<Args> parseArgs(int argc, char** argv) {
    if (argc < 4 || argc > 5) {
        return Error() << "unexpected number of arguments";
    }

    auto key = readBinaryFile(argv[1]);
    if (!key.ok()) {
        return Error() << "Failed to read key file: " << key.error();
    }

    auto input = readBinaryFile(argv[2]);
    if (!input.ok()) {
        return Error() << "Failed to read input file: " << input.error();
    }

    std::string output_path = argv[3];
    size_t sector_size = kDefaultSectorSize;
    if (argc >= 5) {
        if (!android::base::ParseUint(argv[4], &sector_size)) {
            return Error() << "Failed to parse sector size: " << argv[4];
        }
        if (sector_size < 512 || sector_size > 4096) {
            return Error() << "Sector size must be between 512 and 4096 bytes";
        }
        if (!std::has_single_bit(sector_size)) {
            return Error() << "Sector size must be a power of two";
        }
    }

    Args args = {.key = *key,
                 .input = *input,
                 .output_path = output_path,
                 .sector_size = sector_size};
    return args;
}

// Implements the aes-xts-plain64 encryption algorithm used in dm-crypt.
//
// Key (64 bytes): K1 || K2
//   K1 (32 bytes): AES-256 key for data encryption.
//   K2 (32 bytes): AES-256 key for tweak generation.
//
// Sector IV (16 bytes): Composed of the sector number in little-endian format.
//                       le64(sector_number) || 0^64
//
// Per sector encryption flow:
//
//       Sector IV
//           |
//     [AES-ENC(K2)]
//           |
//           +---------------------> (* alpha) ------------------> ...
//           | Tweak0                    | Tweak1
//           +-------------+             +-------------+
//           |             |             |             |
//           v             v             v             v
// Plain0->(XOR)           |   Plain1->(XOR)           |
//           |             |             |             |
//     [AES-ENC(K1)]       |       [AES-ENC(K1)]       |
//           |             |             |             |
//           v             v             v             v
//         (XOR) <---------+           (XOR) <---------+
//           |                           |
//           v                           v
//        Cipher0                     Cipher1
//
// Per-block processing inside a sector:
//   T0 = AES-ECB(K2, IV)
//   Ti = T0 * (alpha^i) in GF(2^128). The field is defined by the polynomial
//        x^128 + x^7 + x^2 + x + 1. The multiplication by alpha is implemented
//        as a left-shift by 1 on the little-endian representation of the tweak.
//        If a carry occurs, the lowest byte is XORed with 0x87.
//
//   The ciphertext for block i (Ci) is calculated as:
//   Ci = AES-ECB(K1, Pi XOR Ti) XOR Ti
//
// Note: Ciphertext stealing is not needed because the input size is always a multiple
// of the sector size, ensuring no partial blocks at the end of the input.
//
// Ref: https://en.wikipedia.org/wiki/Disk_encryption_theory#XTS
Result<std::vector<uint8_t>> aesXtsPlain64Encrypt(const std::vector<uint8_t>& key,
                                                  const std::vector<uint8_t>& input,
                                                  size_t sector_size) {
    if (key.size() != 64) {
        return Error() << "Key size must be 64 bytes for AES-XTS";
    }

    if (sector_size % kAesBlockSize != 0) {
        return Error() << "Sector size must be a multiple of " << kAesBlockSize;
    }

    if (input.size() % sector_size != 0) {
        return Error() << "Input size must be a multiple of sector size";
    }

    bssl::UniquePtr<EVP_CIPHER_CTX> ctx(EVP_CIPHER_CTX_new());
    if (!ctx) {
        return Error() << "EVP_CIPHER_CTX_new failed";
    }

    auto ecb_encrypt = [&ctx](const uint8_t* key, const uint8_t* in, uint8_t* out,
                              size_t len) -> Result<void> {
        if (EVP_EncryptInit_ex(ctx.get(), EVP_aes_256_ecb(), /*impl=*/nullptr, key,
                               /*iv=*/nullptr) != 1) {
            return Error() << "EVP_EncryptInit_ex failed";
        }
        EVP_CIPHER_CTX_set_padding(ctx.get(), 0);
        int outl = 0;
        if (EVP_EncryptUpdate(ctx.get(), out, &outl, in, len) != 1 || outl != len) {
            return Error() << "EVP_EncryptUpdate failed";
        }
        return {};
    };

    // Applies the evolving XTS mask to the data in-place.
    // The mask of i-th block is T0 * (alpha^i) in GF(2^128).
    auto apply_xts_mask = [](const uint8_t* tweak, std::span<uint8_t> data) {
        uint8_t mask[kAesBlockSize];
        memcpy(mask, tweak, kAesBlockSize);

        for (size_t i = 0; i < data.size(); i += kAesBlockSize) {
            for (size_t j = 0; j < kAesBlockSize; ++j) {
                data[i + j] ^= mask[j];
            }

            // mask *= alpha in GF(2^128), little-endian representation
            uint8_t carry = 0;
            for (size_t j = 0; j < 16; ++j) {
                uint8_t new_carry = mask[j] >> 7;
                mask[j] = (mask[j] << 1) | carry;
                carry = new_carry;
            }

            if (carry != 0) {
                // GF(2^128) with modulus x^128 + x^7 + x^2 + x + 1
                mask[0] ^= 0b1000'0111;
            }
        }
    };

    const uint8_t* k1 = key.data();
    const uint8_t* k2 = key.data() + 32;

    size_t num_sectors = input.size() / sector_size;
    std::vector<uint8_t> output(input.size());
    for (size_t sector = 0; sector < num_sectors; ++sector) {
        // IV = le64(sector) || 0^64
        uint8_t iv[kAesBlockSize] = {};
        for (size_t i = 0; i < 8; ++i) {
            iv[i] = static_cast<uint8_t>((sector >> (i * 8)) & 0xFF);
        }

        // Tweak0 = AES-ECB(K2, IV)
        uint8_t tweak[kAesBlockSize] = {};
        auto tweak_result = ecb_encrypt(k2, iv, tweak, sizeof(tweak));
        if (!tweak_result.ok()) {
            return Error() << "Failed to generate tweak: " << tweak_result.error();
        }

        // Applies the pre-encryption mask.
        size_t offset = sector * sector_size;
        std::span<uint8_t> sector_data(output.data() + offset, sector_size);
        memcpy(sector_data.data(), input.data() + offset, sector_size);
        apply_xts_mask(tweak, sector_data);

        // Encrypts the masked sector data.
        auto encrypt_result = ecb_encrypt(k1, sector_data.data(), sector_data.data(), sector_size);
        if (!encrypt_result.ok()) {
            return Error() << "Failed to encrypt sector: " << encrypt_result.error();
        }

        // Applies the post-encryption mask.
        apply_xts_mask(tweak, sector_data);
    }

    return output;
}

} // namespace

int main(int argc, char** argv) {
    if (argc < 4 || argc > 5) {
        std::cerr << "Usage: " << argv[0] << " <key_file> <input_file> <output_file> [sector_size]"
                  << std::endl;
        return EXIT_FAILURE;
    }

    auto args = parseArgs(argc, argv);
    if (!args.ok()) {
        std::cerr << "Failed to parse arguments: " << args.error() << std::endl;
        return EXIT_FAILURE;
    }

    auto output = aesXtsPlain64Encrypt(args->key, args->input, args->sector_size);
    if (!output.ok()) {
        std::cerr << "Failed to encrypt input: " << output.error() << std::endl;
        return EXIT_FAILURE;
    }

    if (!writeBinaryFile(*output, args->output_path)) {
        std::cerr << "Failed to write output: " << args->output_path << std::endl;
        return EXIT_FAILURE;
    }

    return 0;
}
