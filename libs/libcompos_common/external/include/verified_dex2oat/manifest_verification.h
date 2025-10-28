/*
 * Copyright (C) 2025 The Android Open Source Project
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

#include <stdbool.h>
#include <stdint.h>
#include <sys/cdefs.h>

#pragma once
__BEGIN_DECLS

typedef struct AVerifiedDex2Oat_Verifier_ExpectationContext
        AVerifiedDex2Oat_Verifier_ExpectationContext;

/**
 * Represents the status codes returned by the AVerifiedDex2Oat_Verifier API.
 * Introduced in API Level 38.
 */
typedef enum AVerifiedDex2Oat_Verifier_Status : int32_t {
    /**
     * Success, manifest does not violate any rules.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_SUCCESS = 0,

    /**
     * Failure, manifest contains an argument not covered by any rule.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_UNEXPECTED_ARGUMENT = -1,

    /**
     * Failure, manifest signature invalid.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_INVALID_MANIFEST_SIGNATURE = -2,

    /**
     * Failure, unable to retrieve CompOS app VM public key.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_UNABLE_TO_RETRIEVE_PUBLIC_KEY = -3,

    /**
     * Failure, manifest could not be opened.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_UNABLE_TO_OPEN_MANIFEST = -4,

    /**
     * Failure, file failed provenance check due to mismatched fs-verity digest.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_FILE_MISMATCHED_FS_VERITY_DIGEST = -5,

    /**
     * Failure, file failed provenance check due to missing fs-verity
     * digest.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_FILE_MISSING_FS_VERITY_DIGEST = -6,

    /**
     * Failure, compiler argument exact match failed.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_EXACT_MATCH_FAILED = -9,

    /**
     * Failure, compiler argument prohibited argument failed.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_PROHIBITED_ARGUMENT = -10,

    /**
     * Failure, one or more arguments provided were invalid.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_BAD_ARGS = -11,

    /**
     * Failure, expectation context is in an invalid state for the operation.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_INVALID_STATE = -12,

    /**
     * Failure, General Failure.
     * Introduced in API Level 38.
     */
    AVERIFIED_DEX2OAT_VERIFIER_FAILURE = -255,
} AVerifiedDex2Oat_Verifier_Status;

/**
 * Creates an opaque expectation context that will be used to verify CompOS compiler arguments.
 *
 * Initializes an empty expectation context that compiler arguments to be matched can be added to.
 * Expectations are positional and shall correspond to one compiler argument.
 * In the case where a compiler argument does not have an associated expectation
 * then verification will fail; this indicates that the compiler argument wasn't
 * analyzed for security impact.
 * AVerifiedDex2Oat_Verifier_Expectation_destroy must be called to free the associated memory when
 * it is no longer needed.
 *
 * @return
 *   - On success this will point to a pointer for an expectation context. This context
 * should only be destroyed by `AVerifiedDex2Oat_Verifier_Expectation_destroy`.
 *   - On failure this will return NULL.
 */
// clang-format off
AVerifiedDex2Oat_Verifier_ExpectationContext* _Nullable
        AVerifiedDex2Oat_Verifier_Expectation_create(void) __INTRODUCED_IN(38);
// clang-format on

/**
 * Destroy an expectation context created by AVerifiedDex2Oat_Verifier_Expectation_create.
 *
 * This function takes a pointer to a expectation context and frees it.
 *
 * @param ctx The expectation context that was created by
 * `AVerifiedDex2Oat_Verifier_Expectation_create`
 */
void AVerifiedDex2Oat_Verifier_Expectation_destroy(
        AVerifiedDex2Oat_Verifier_ExpectationContext* _Nonnull ctx) __INTRODUCED_IN(38);

/**
 * Adds an exact match argument rule to the expectation context.
 *
 * The exact match will be used to compare against the CompOS manifest. When there is no file
 * descriptors (fds) a simple string comparison will occur during verification. When there are fds
 * the fs-verity digest will be used to verify that the files match against the manifest files.
 *
 * @param ctx The expectation context that was created by
 * `AVerifiedDex2Oat_Verifier_Expectation_create`
 * @param formatString A UTF-8 encoded null terminated string (where null is allowed only for string
 * termination) to match against in the manifest.
 *   `!` serves as an escapable placeholder for `fd` in these strings that is positionally matched.
 * The number of `!` in the string should should equal `fd_count`.
 * @param fds The ordered file descriptors referenced in formatString; for fileless exact matches
 * this will be NULL.
 * @param fdCount 0 for fileless exact matches, otherwise the number of fds in fdCount.
 *
 * @return
 *   - `AVERIFIED_DEX2OAT_VERIFIER_SUCCESS` on success.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_BAD_ARGS` if any fd in fds is invalid, the number of
 *     unescaped '!' placeholders in formatString does not match fdCount, or if formatString cannot
 *     be parsed as UTF-8.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_FILE_MISSING_FS_VERITY_DIGEST` if any fd does not have an
 * fs-verity digest.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_FAILURE` for other errors.
 */
AVerifiedDex2Oat_Verifier_Status AVerifiedDex2Oat_Verifier_Expectation_addExactMatchRule(
        AVerifiedDex2Oat_Verifier_ExpectationContext* _Nonnull ctx,
        const char* _Nonnull formatString, const int* _Nullable fds, size_t fdCount)
        __INTRODUCED_IN(38);

/** Adds a disallowed argument rule to the expectation context.
 *
 * If the added compiler argument is found in the manifest during verification it will cause an
 * immediate verification failure.
 *
 * @param ctx The expectation context that was created by
 * `AVerifiedDex2Oat_Verifier_Expectation_create`
 * @param compilerArg The prohibited argument as a null-terminated UTF-8 string (where null is
 * allowed only for string termination).
 * @param isPrefix A prohibited compiler argument is detected if arg is a prefix of that argument.
 *
 * @return
 *   - `AVERIFIED_DEX2OAT_VERIFIER_SUCCESS` on success.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_BAD_ARGS` if compilerArg cannot be parsed.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_FAILURE` for other errors.
 */
AVerifiedDex2Oat_Verifier_Status AVerifiedDex2Oat_Verifier_Expectation_addDisallowedArgumentRule(
        AVerifiedDex2Oat_Verifier_ExpectationContext* _Nonnull ctx,
        const char* _Nonnull compilerArg, bool isPrefix) __INTRODUCED_IN(38);

/**
 * Adds an ignored argument rule to the expectation context.
 *
 * The validation passes whether or not the argument is present or absent. The purpose of this is to
 * signal that the a flag was analyzed and considered non security impacting.
 *
 * @param ctx The expectation context that was created by
 * `AVerifiedDex2Oat_Verifier_Expectation_create`
 * @param compilerArg The don't care argument as a null terminated UTF-8 string (where null is
 * allowed only for string termination).
 * @param isPrefix A successful match if the argument is a prefix of the compiler argument in the
 * manifest.
 *
 * @return
 *   - `AVERIFIED_DEX2OAT_VERIFIER_SUCCESS` on success.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_BAD_ARGS` if compilerArg cannot be parsed.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_FAILURE` for other errors.
 */
AVerifiedDex2Oat_Verifier_Status AVerifiedDex2Oat_Verifier_Expectation_addIgnoredArgumentRule(
        AVerifiedDex2Oat_Verifier_ExpectationContext* _Nonnull ctx,
        const char* _Nonnull compilerArg, bool isPrefix) __INTRODUCED_IN(38);

/**
 * Generates a compound rule where compilerArg is combined with the previous match rule.
 *
 * This should only be used when the most recently added expectation Rule is an ignored or
 * disallowed Rule. This expresses the intent that two arguments must be matched as a whole.
 * Example:
 * AVerifiedDex2Oat_Verifier_Expectation_addDisallowedArgumentRule(ctx, "--runtime-arg", false)
 * AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(ctx, "--foo", true)
 * Would yield a prohibited expectation that would match the following compiler args
 * ["--runtime-arg", "--foo-bar"]
 * ["--runtime-arg", "--foo"]
 *
 * But would not match these
 * ["--runtime-arg --foo-bar"]
 * ["--runtime-arg", "--bar"]
 *
 * @param ctx The expectation context that was created by
 * `AVerifiedDex2Oat_Verifier_Expectation_create`
 * @param compilerArg A UTF-8 encoded null terminated string (where null is allowed only for string
 * termination) to match against in the manifest.
 *   `!` serves as an escapable placeholder for `fd` in these strings.
 * @param isPrefix A successful match if the argument is a prefix of the compiler argument in the
 * manifest.
 *
 * @return
 *   - `AVERIFIED_DEX2OAT_VERIFIER_SUCCESS` on success.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_BAD_ARGS` if compilerArg cannot be parsed.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_INVALID_STATE` if there are no previous rules or the most
 * recently added rule is an exact match.
 */
AVerifiedDex2Oat_Verifier_Status AVerifiedDex2Oat_Verifier_Expectation_combineWithPreviousMatchRule(
        AVerifiedDex2Oat_Verifier_ExpectationContext* _Nonnull ctx,
        const char* _Nonnull compilerArg, bool isPrefix) __INTRODUCED_IN(38);

/**
 * Verifies all argument rules added the expectation context match against the manifest proto.
 *
 * This method will iterate through the manifest proto and compare against the given expectations
 * one-by-one. Results depend on type of argument found:
 *   - Ignored: move onto next element.
 *   - Disallowed: fail verification immediately.
 *   - Exact: verify argument (string comparison or fs-verity hash matches) and continue to next
 *     element.
 *   - Unknown: If the argument does not match any expectation fail verification immediately.
 *
 * If the entire manifest is verified it is considered "safe".
 * The result of this method will update the string value returned by
 * `AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage`.
 *
 * @param ctx The expectation context that was created by
 *        `AVerifiedDex2Oat_Verifier_Expectation_create`.
 * @param manifestPath A null terminated UTF-8 string (where null is allowed only for string
 *        termination) containing the path to the manifest file that contains the AOT compiler
 *        arguments recorded by CompOS.
 *
 * @return
 *   - `AVERIFIED_DEX2OAT_VERIFIER_SUCCESS`: success, all arguments in manifest matched rules.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_UNEXPECTED_ARGUMENT`: failure, manifest contains an argument not
 *      covered by any rule.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_INVALID_MANIFEST_SIGNATURE`: failure, manifest signature invalid.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_UNABLE_TO_RETRIEVE_PUBLIC_KEY`: failure, unable to retrieve
 *     CompOS app VM public key.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_UNABLE_TO_OPEN_MANIFEST`: failure, manifest could not be opened.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_FILE_MISMATCHED_FS_VERITY_DIGEST`: file failed
 *     provenance check due to mismatched fs-verity digest.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_FILE_MISSING_FS_VERITY_DIGEST`: file failed provenance check
 *     due to missing fs-verity digest.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_EXACT_MATCH_FAILED`: compiler argument exact match failed.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_PROHIBITED_ARGUMENT`: compiler argument prohibited argument
 *     found.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_BAD_ARGS`: manifestPath not UTF-8 encoded.
 *   - `AVERIFIED_DEX2OAT_VERIFIER_FAILURE`: failure, some other reason like the proto being
 *     malformed.
 */
AVerifiedDex2Oat_Verifier_Status AVerifiedDex2Oat_Verifier_Expectation_verify(
        AVerifiedDex2Oat_Verifier_ExpectationContext* _Nonnull ctx,
        const char* _Nonnull manifestPath) __INTRODUCED_IN(38);

/**
 * Returns NULL or a UTF-8 encoded null terminated string containing the reason for verification
 * failure.
 *
 * The lifetime of the returned string is tied to the expectation context. It will be overwritten by
 * subsequent calls to `AVerifiedDex2Oat_Verifier_Expectation_verify`.
 *
 * @param ctx The expectation context that was created by
 *        `AVerifiedDex2Oat_Verifier_Expectation_create`.
 * @return A pointer to a UTF-8 null terminated string containing the reason for error, or NULL if
 *         there is no error (verify method was not called or it returned
 *         AVERIFIED_DEX2OAT_VERIFIER_SUCCESS).
 */
const char* _Nullable AVerifiedDex2Oat_Verifier_Expectation_getErrorMessage(
        AVerifiedDex2Oat_Verifier_ExpectationContext* _Nonnull ctx) __INTRODUCED_IN(38);

__END_DECLS
