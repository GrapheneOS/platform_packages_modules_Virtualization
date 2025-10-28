// Copyright (C) 2025 The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#include <android-base/file.h>
#include <android-base/hex.h>
#include <android/log.h>
#include <fcntl.h>
#include <gtest/gtest.h>
#include <linux/fsverity.h>
#include <openssl/evp.h>
#include <openssl/pem.h>
#include <openssl/sha.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <cerrno>
#include <cstdint>
#include <cstdio> // For remove()
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <map>
#include <sstream>
#include <vector>

#include "libfsverity.h"
#include "proto/manifest.pb.h"
#include "verified_dex2oat/manifest_verification.h"

using com::android::compos::proto::Signature;
using com::android::compos::proto::SignatureAlgorithm;
using SignedManifest = com::android::compos::proto::Signature_SignedManifest;
using SecureCompileManifest =
        com::android::compos::proto::Signature_SignedManifest_SecureCompileManifest;
using CompilerArgument = com::android::compos::proto::
        Signature_SignedManifest_SecureCompileManifest_CompilerArgument;

constexpr char kTestDir[] = "/data/local/tmp/manifest_verification_test";
constexpr char kPublicKeyDir[] = "/data/misc/apexdata/com.android.compos";
constexpr char kPublicKeyName[] = "compos_dex2oat_key";

// Helper read function for libfsverity_compute_digest.
int read_callback(void* fd, void* buf, size_t count) {
    ssize_t res = read(*(int*)fd, buf, count);
    if (res < 0) return -errno;
    return 0;
}

// Helper function to generate Ed25519 keypair and write public key to file.
EVP_PKEY* GenerateKeyPairAndWritePublicKey(const std::string& public_key_path) {
    EVP_PKEY* pkey = EVP_PKEY_new();
    EVP_PKEY_CTX* ctx = EVP_PKEY_CTX_new_id(EVP_PKEY_ED25519, nullptr);
    if (!ctx || EVP_PKEY_keygen_init(ctx) <= 0 || EVP_PKEY_keygen(ctx, &pkey) <= 0) {
        EVP_PKEY_CTX_free(ctx);
        EVP_PKEY_free(pkey);
        return nullptr;
    }
    EVP_PKEY_CTX_free(ctx);

    FILE* fp = fopen(public_key_path.c_str(), "wb");
    if (!fp) {
        EVP_PKEY_free(pkey);
        return nullptr;
    }
    size_t len = 32;
    uint8_t pubkey[32];
    if (EVP_PKEY_get_raw_public_key(pkey, pubkey, &len) == 0) {
        fclose(fp);
        EVP_PKEY_free(pkey);
        return nullptr;
    }
    fwrite(pubkey, 1, len, fp);
    fclose(fp);

    return pkey;
}

struct Argument {
    std::string argument;
    std::vector<std::vector<uint8_t>> hashes;
};

class ManifestVerificationTest : public ::testing::Test {
protected:
    AVerifiedDex2Oat_Verifier_ExpectationContext* ctx_ = nullptr;
    struct libfsverity_merkle_tree_params params_;
    std::string public_key_path_;
    EVP_PKEY* pkey_ = nullptr;

    void SetUp() override {
        std::error_code ec;
        // Create the test directory for temporary test files.
        std::filesystem::create_directory(kTestDir, ec);
        ASSERT_FALSE(ec) << "Failed to create test directory: " << ec.message();

        // Create the directory for the public key.
        std::filesystem::create_directories(kPublicKeyDir, ec);
        ASSERT_FALSE(ec) << "Failed to create public key directory: " << ec.message();

        public_key_path_ = std::string(kPublicKeyDir) + "/" + kPublicKeyName;
        pkey_ = GenerateKeyPairAndWritePublicKey(public_key_path_);
        ASSERT_NE(pkey_, nullptr);

        ctx_ = AVerifiedDex2Oat_Verifier_Expectation_create();
        ASSERT_NE(ctx_, nullptr);

        memset(&params_, 0, sizeof(params_));
        params_.version = 1;
        params_.hash_algorithm = FS_VERITY_HASH_ALG_SHA256;
        params_.block_size = 4096;
    }

    void TearDown() override {
        if (ctx_) {
            AVerifiedDex2Oat_Verifier_Expectation_destroy(ctx_);
        }
        if (pkey_) {
            EVP_PKEY_free(pkey_);
        }

        std::error_code ec;
        // Clean up the test directory.
        std::filesystem::remove_all(kTestDir, ec);
        ASSERT_FALSE(ec) << "Failed to remove test directory: " << ec.message();

        // Clean up the public key file and directory.
        std::filesystem::remove(public_key_path_, ec);
        std::filesystem::remove(kPublicKeyDir, ec);
    }
};

// Helper to get fs-verity digest using ioctl.
std::vector<uint8_t> get_fsverity_digest(int fd) {
    alignas(struct fsverity_digest) char buf[sizeof(struct fsverity_digest) + 64];
    struct fsverity_digest* digest = (struct fsverity_digest*)buf;
    digest->digest_size = 64;

    if (ioctl(fd, FS_IOC_MEASURE_VERITY, digest) < 0) {
        fprintf(stderr, "Failed to measure fs-verity digest: %s\n", strerror(errno));
        return {};
    }

    return std::vector<uint8_t>(digest->digest, digest->digest + digest->digest_size);
}

// Helper class to create fake manifest protos to verify.
Signature CreateFakeManifestProto(const std::vector<Argument>& args, EVP_PKEY* pkey) {
    Signature sig_proto;
    SignedManifest* signed_manifest = sig_proto.mutable_compos_signed_manifest();
    signed_manifest->set_algorithm(SignatureAlgorithm::ED25519);

    SecureCompileManifest* manifest = signed_manifest->mutable_manifest();
    for (const auto& entry : args) {
        const std::string& arg_str = entry.argument;
        const std::vector<std::vector<uint8_t>>& hashes = entry.hashes;

        CompilerArgument* comp_arg = manifest->add_compiler_arguments();
        comp_arg->set_compiler_flag(arg_str);

        for (const auto& hash : hashes) {
            auto* file_info = comp_arg->add_file_info();
            file_info->set_verity_digest("sha256-" +
                                         android::base::HexString(hash.data(), hash.size()));
        }
    }

    // Sign the manifest.
    std::string manifest_bytes;
    manifest->SerializeToString(&manifest_bytes);

    unsigned char hash[SHA256_DIGEST_LENGTH];
    SHA256((const unsigned char*)manifest_bytes.data(), manifest_bytes.length(), hash);

    const std::string prefix = "compos_secure_compilation";
    std::vector<uint8_t> msg_to_sign(prefix.begin(), prefix.end());
    msg_to_sign.insert(msg_to_sign.end(), hash, hash + SHA256_DIGEST_LENGTH);

    EVP_MD_CTX* mdctx = EVP_MD_CTX_new();
    if (!mdctx) {
        return sig_proto;
    }

    if (EVP_DigestSignInit(mdctx, nullptr, nullptr, nullptr, pkey) <= 0) {
        EVP_MD_CTX_free(mdctx);
        return sig_proto;
    }

    size_t sig_len = EVP_PKEY_size(pkey);
    std::vector<unsigned char> sig(sig_len);

    if (EVP_DigestSign(mdctx, sig.data(), &sig_len, (const unsigned char*)msg_to_sign.data(),
                       msg_to_sign.size()) <= 0) {
        EVP_MD_CTX_free(mdctx);
        return sig_proto;
    }

    signed_manifest->set_signature(sig.data(), sig_len);

    EVP_MD_CTX_free(mdctx);

    return sig_proto;
}

// Helper to write a proto to a file.
bool WriteProtoToFile(const Signature& proto, const std::string& file_path) {
    std::ofstream os(file_path, std::ios::binary);
    if (!os.is_open()) {
        return false;
    }
    return proto.SerializeToOstream(&os);
}

TEST_F(ManifestVerificationTest, CreateThenDestroy) {
    // Setup and Teardown handle this.
    SUCCEED();
}

TEST_F(ManifestVerificationTest, VerifySuccess) {
    // Exact match.
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(ctx_, "-Xexact1", nullptr, 0),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);
    // Ignore prefix.
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(ctx_, "--ignore-me",
                                                                           true),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    // Exact match with FDs.
    std::string tmp_dex1 = std::string(kTestDir) + "/manifest_test_1";
    std::string tmp_dex2 = std::string(kTestDir) + "/manifest_test_2";

    // Create and write to tmp_dex1
    std::ofstream ofs1(tmp_dex1);
    ASSERT_TRUE(ofs1.is_open());
    ofs1 << "test1";
    ofs1.close();

    // Create and write to tmp_dex2
    std::ofstream ofs2(tmp_dex2, std::ios::binary);
    ASSERT_TRUE(ofs2.is_open());
    ofs2 << "test2";
    ofs2.close();

    // Reopen files in read-only mode
    int fd1 = open(tmp_dex1.c_str(), O_RDONLY);
    int fd2 = open(tmp_dex2.c_str(), O_RDONLY);
    ASSERT_GE(fd1, 0);
    ASSERT_GE(fd2, 0);

    // Setup fsverity with temp files.
    struct libfsverity_merkle_tree_params params1 = params_;
    struct stat st1;
    ASSERT_EQ(fstat(fd1, &st1), 0);
    params1.file_size = st1.st_size;
    ASSERT_EQ(libfsverity_enable(fd1, &params1), 0);

    struct libfsverity_merkle_tree_params params2 = params_;
    struct stat st2;
    ASSERT_EQ(fstat(fd2, &st2), 0);
    params2.file_size = st2.st_size;
    ASSERT_EQ(libfsverity_enable(fd2, &params2), 0);

    std::vector<uint8_t> hash1 = get_fsverity_digest(fd1);
    std::vector<uint8_t> hash2 = get_fsverity_digest(fd2);
    ASSERT_FALSE(hash1.empty());
    ASSERT_FALSE(hash2.empty());

    int fds[] = {fd1, fd2};
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(ctx_, "--dex-files=!,!", fds,
                                                                      2),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    std::vector<Argument> args = {{"-Xexact1", {}},
                                  {"--ignore-me-please", {}},
                                  {"--dex-files=!,!", {hash1, hash2}}};

    Signature manifest_proto = CreateFakeManifestProto(args, pkey_);
    std::string manifest_path = std::string(kTestDir) + "/success_manifest.bin";
    ASSERT_TRUE(WriteProtoToFile(manifest_proto, manifest_path));

    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);
    ASSERT_EQ(status, AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);
    ASSERT_EQ(error_message, nullptr);
    close(fd1);
    close(fd2);
}

TEST_F(ManifestVerificationTest, VerifyFailExactMatch) {
    // Exact match.
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(ctx_, "-Xexact1", nullptr, 0),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    std::vector<Argument> args = {{"-Xexact2", {}}}; // Mismatch

    Signature manifest_proto = CreateFakeManifestProto(args, pkey_);
    std::string manifest_path = std::string(kTestDir) + "/exact_fail_manifest.bin";
    ASSERT_TRUE(WriteProtoToFile(manifest_proto, manifest_path));

    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);
    ASSERT_EQ(status, AVERIFIED_DEX2OAT_VERIFIER_EXACT_MATCH_FAILED);
    ASSERT_NE(error_message, nullptr);
    ASSERT_STREQ(error_message, "Rule not matched: expected -Xexact1, got -Xexact2");
}

TEST_F(ManifestVerificationTest, VerifyFailExactMatchNotFound) {
    // Exact match.
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(ctx_, "-Xexact1", nullptr, 0),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    // Empty manifest.
    std::vector<Argument> args = {};

    Signature manifest_proto = CreateFakeManifestProto(args, pkey_);
    std::string manifest_path = std::string(kTestDir) + "/exact_not_found_manifest.bin";
    ASSERT_TRUE(WriteProtoToFile(manifest_proto, manifest_path));
    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);
    ASSERT_EQ(status, AVERIFIED_DEX2OAT_VERIFIER_EXACT_MATCH_FAILED);
    ASSERT_NE(error_message, nullptr);
    ASSERT_STREQ(error_message, "Rule expects 1 arguments, but only 0 remaining in manifest");
}

TEST_F(ManifestVerificationTest, VerifyFailDisallowed) {
    // Disallow.
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addDisallowedArgumentRule(ctx_,
                                                                              "--disallowed-arg",
                                                                              false),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    std::vector<Argument> args = {{"--disallowed-arg", {}}}; // Contains disallowed

    Signature manifest_proto = CreateFakeManifestProto(args, pkey_);
    std::string manifest_path = std::string(kTestDir) + "/disallowed_manifest.bin";
    ASSERT_TRUE(WriteProtoToFile(manifest_proto, manifest_path));

    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);
    ASSERT_EQ(status, AVERIFIED_DEX2OAT_VERIFIER_PROHIBITED_ARGUMENT);
    ASSERT_NE(error_message, nullptr);
}

TEST_F(ManifestVerificationTest, VerifyInvalidSignature) {
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(ctx_, "-Xexact1", nullptr, 0),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    std::vector<Argument> args = {{"-Xexact1", {}}};

    Signature manifest_proto = CreateFakeManifestProto(args, pkey_);
    std::string manifest_path = std::string(kTestDir) + "/invalid_signature_manifest.bin";
    ASSERT_TRUE(WriteProtoToFile(manifest_proto, manifest_path));

    // Tamper with the file
    std::ofstream ofs(manifest_path, std::ios::app | std::ios::binary);
    ofs.write("\0", 1);
    ofs.close();

    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);

    // It might fail parsing or signature check.
    ASSERT_NE(status, AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);
}

TEST_F(ManifestVerificationTest, VerifyRuleConsumption) {
    // Add ONE ignored rule
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(ctx_, "--foo", false),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    // Manifest has TWO arguments
    std::vector<Argument> args = {{"--foo", {}}, {"--foo", {}}};

    Signature manifest_proto = CreateFakeManifestProto(args, pkey_);
    std::string manifest_path = std::string(kTestDir) + "/rule_consumption_manifest.bin";
    ASSERT_TRUE(WriteProtoToFile(manifest_proto, manifest_path));

    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);

    ASSERT_EQ(status, AVERIFIED_DEX2OAT_VERIFIER_UNEXPECTED_ARGUMENT);
    ASSERT_NE(error_message, nullptr);
}

TEST_F(ManifestVerificationTest, VerifyCombinedRules) {
    // Rule: --arg1.
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(ctx_, "--arg1", false),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);
    // Combine with Rule: --arg2.
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(ctx_, "--arg2",
                                                                                 false),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    std::vector<Argument> args = {{"--arg1", {}}, {"--arg2", {}}};

    Signature manifest_proto = CreateFakeManifestProto(args, pkey_);
    std::string manifest_path = std::string(kTestDir) + "/combined_rules_manifest.bin";
    ASSERT_TRUE(WriteProtoToFile(manifest_proto, manifest_path));

    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);

    ASSERT_EQ(status, AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);
    ASSERT_EQ(error_message, nullptr);
}

// Structure definitions from authfs/src/fsverity/metadata/metadata.hpp
enum class FSVERITY_SIGNATURE_TYPE : uint32_t {
    NONE = 0,
    PKCS7 = 1,
    RAW = 2,
};

struct fsverity_metadata_header {
    uint32_t version;
    struct fsverity_descriptor descriptor;
    FSVERITY_SIGNATURE_TYPE signature_type;
    uint32_t signature_size;
} __attribute__((packed));

// Helper to write Merkle tree blocks to a file.
int merkle_tree_block_callback(void* ctx, const void* block, size_t size, uint64_t offset) {
    int fd = *(int*)ctx;
    // Merkle tree starts at 4K offset in .fsv_meta.
    if (pwrite(fd, block, size, 4096 + offset) != (ssize_t)size) {
        return -errno;
    }
    return 0;
}

int descriptor_callback(void* ctx, const void* descriptor, size_t size) {
    auto* desc_out = static_cast<std::vector<uint8_t>*>(ctx);
    desc_out->assign(static_cast<const uint8_t*>(descriptor),
                     static_cast<const uint8_t*>(descriptor) + size);
    return 0;
}

// Used to test falling back to the fsv meta file.
void CreateFsvMetaFile(const std::string& data_file, const std::string& meta_file,
                       struct libfsverity_merkle_tree_params& params,
                       std::vector<uint8_t>& out_digest) {
    int data_fd = open(data_file.c_str(), O_RDONLY);
    ASSERT_GE(data_fd, 0);

    int meta_fd = open(meta_file.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0644);
    ASSERT_GE(meta_fd, 0);

    std::vector<uint8_t> captured_descriptor;
    struct libfsverity_metadata_callbacks callbacks = {};
    struct {
        int fd;
        std::vector<uint8_t>* descriptor;
    } cb_ctx = {meta_fd, &captured_descriptor};

    callbacks.ctx = &cb_ctx;
    callbacks.merkle_tree_block = [](void* ctx, const void* block, size_t size, uint64_t offset) {
        auto* c = static_cast<decltype(&cb_ctx)>(ctx);
        return merkle_tree_block_callback(&c->fd, block, size, offset);
    };
    callbacks.descriptor = [](void* ctx, const void* descriptor, size_t size) {
        auto* c = static_cast<decltype(&cb_ctx)>(ctx);
        return descriptor_callback(c->descriptor, descriptor, size);
    };

    params.metadata_callbacks = &callbacks;

    struct libfsverity_digest* digest_struct = nullptr;
    ASSERT_EQ(libfsverity_compute_digest(&data_fd, read_callback, &params, &digest_struct), 0);
    out_digest.assign(digest_struct->digest, digest_struct->digest + digest_struct->digest_size);

    ASSERT_EQ(captured_descriptor.size(), sizeof(fsverity_descriptor));

    struct fsverity_metadata_header header = {};
    header.version = 1;
    memcpy(&header.descriptor, captured_descriptor.data(), sizeof(fsverity_descriptor));
    header.signature_type = FSVERITY_SIGNATURE_TYPE::NONE;
    header.signature_size = 0;

    ASSERT_EQ(pwrite(meta_fd, &header, sizeof(header), 0), (ssize_t)sizeof(header));

    close(data_fd);
    close(meta_fd);
    free(digest_struct);
}

TEST_F(ManifestVerificationTest, VerifySuccessWithFsvMetaFallback) {
    std::string test_file = std::string(kTestDir) + "/fallback_test_file";
    std::string test_meta = test_file + ".fsv_meta";

    // Create a 4K file of dummy data.
    std::string dummy_data(4096, 0xAB);
    ASSERT_TRUE(android::base::WriteStringToFile(dummy_data, test_file));

    struct libfsverity_merkle_tree_params params = params_;
    params.file_size = dummy_data.size();

    std::vector<uint8_t> expected_hash;
    CreateFsvMetaFile(test_file, test_meta, params, expected_hash);

    int fd = open(test_file.c_str(), O_RDONLY);
    ASSERT_GE(fd, 0);

    // addExactMatchRule should succeed by falling back to .fsv_meta.
    int fds[] = {fd};
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(ctx_, "--file=!", fds, 1),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    std::vector<Argument> args = {{"--file=!", {expected_hash}}};
    Signature manifest_proto = CreateFakeManifestProto(args, pkey_);
    std::string manifest_path = std::string(kTestDir) + "/fallback_manifest.bin";
    ASSERT_TRUE(WriteProtoToFile(manifest_proto, manifest_path));

    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);
    ASSERT_EQ(status, AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);
    ASSERT_EQ(error_message, nullptr);

    close(fd);
}

TEST_F(ManifestVerificationTest, VerifyNoManifest) {
    std::string manifest_path = "non_existent_manifest.bin";
    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);
    ASSERT_NE(error_message, nullptr);
}

TEST_F(ManifestVerificationTest, VerifyUnmatchedArgument) {
    // Exact match for -Xexact1
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(ctx_, "-Xexact1", nullptr, 0),
              AVERIFIED_DEX2OAT_VERIFIER_SUCCESS);

    // Manifest has a different argument
    std::vector<Argument> args = {{"-Xunmatched", {}}};

    Signature manifest_proto = CreateFakeManifestProto(args, pkey_);
    std::string manifest_path = std::string(kTestDir) + "/unmatched_arg_manifest.bin";
    ASSERT_TRUE(WriteProtoToFile(manifest_proto, manifest_path));

    AVerifiedDex2Oat_Verifier_Status status =
            AVerifiedDex2Oat_Verifier_Expectation_verify(ctx_, manifest_path.c_str());
    const char* error_message = AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(ctx_);
    ASSERT_EQ(status, AVERIFIED_DEX2OAT_VERIFIER_EXACT_MATCH_FAILED);
    ASSERT_NE(error_message, nullptr);
    ASSERT_STREQ(error_message, "Rule not matched: expected -Xexact1, got -Xunmatched");
}

TEST_F(ManifestVerificationTest, VerifyFailMissingFsVerity) {
    std::string tmp_file1 = std::string(kTestDir) + "/fsverity_test_1";
    std::string tmp_file2 = std::string(kTestDir) + "/fsverity_test_2";

    // Create and write to both files.
    std::ofstream ofs1(tmp_file1);
    ofs1 << "test1";
    ofs1.close();
    std::ofstream ofs2(tmp_file2);
    ofs2 << "test2";
    ofs2.close();

    int fd1 = open(tmp_file1.c_str(), O_RDONLY);
    int fd2 = open(tmp_file2.c_str(), O_RDONLY);
    ASSERT_GE(fd1, 0);
    ASSERT_GE(fd2, 0);

    // Enable fs-verity on ONLY the first file.
    struct libfsverity_merkle_tree_params params1 = params_;
    struct stat st1;
    ASSERT_EQ(fstat(fd1, &st1), 0);
    params1.file_size = st1.st_size;
    ASSERT_EQ(libfsverity_enable(fd1, &params1), 0);

    // Attempt to add a rule with both files. It should fail because fd2 lacks fs-verity.
    int fds[] = {fd1, fd2};
    ASSERT_EQ(AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(ctx_, "--files=!,!", fds, 2),
              AVERIFIED_DEX2OAT_VERIFIER_FILE_MISSING_FS_VERITY_DIGEST);

    close(fd1);
    close(fd2);
}
