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
#include <stdint.h>
#include <sys/cdefs.h>

#pragma once
__BEGIN_DECLS

/**
 * An opaque struct containing the compilation context. This context contains the on success/on
 * failure callbacks, the opaque structs that will be passed to them, a strong reference to the
 * CompOS service, and the parameters used to start compilation.
 * Once `AVerifiedDex2Oat_start` is called on this context, no additional parameters can be
 * added to the context and it should not have start called on it again.
 */
typedef struct AVerifiedDex2Oat_CompilationContext AVerifiedDex2Oat_CompilationContext;

/**
 * An opaque struct containing the user provided and owned opaque data.
 */
typedef struct AVerifiedDex2Oat_SuccessCallbackContext AVerifiedDex2Oat_SuccessCallbackContext;

/**
 * An opaque struct containing information about a successful compilation.
 */
typedef struct AVerifiedDex2Oat_SuccessResultContext AVerifiedDex2Oat_SuccessResultContext;

/**
 * An opaque struct containing the user provided and owned opaque data.
 */
typedef struct AVerifiedDex2Oat_FailureCallbackContext AVerifiedDex2Oat_FailureCallbackContext;

/**
 * An opaque struct containing information about a failed compilation.
 */
typedef struct AVerifiedDex2Oat_FailureResultContext AVerifiedDex2Oat_FailureResultContext;

/**
 * Represents the status codes returned by the AVerifiedDex2Oat APIs.
 * Introduced in API Level 37.
 */
typedef enum AVerifiedDex2Oat_Status : int32_t {
    /** Indicates that the AVerifiedDex2OAT API call completed successfully.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_SUCCESS = 0,

    /** Indicates that one or more arguments provided to the AVerifiedDex2Oat API were invalid.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_BAD_ARGS = -1,

    /**
     * Indicates that the number of file descriptor placeholders in `Dex2OatArg` format string does
     * not match the number of provided file descriptors.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS = -2,

    /** Indicates that a `Dex2OatArg` format string was not valid UTF-8.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_BAD_ARGS_FORMAT_STRING_NOT_UTF8 = -3,

    /** Indicates a generic error occurred when calling into the `composd` service.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_ERROR_CALLING_COMPOS = -10,

    /** Indicates that the `composd` service was not available or timed out during connection.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE = -11,

    /**
     * The operation timed out.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_ERROR_TIMED_OUT = -12,

    /**
     * A general error.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_ERROR_GENERAL = -19,

    /**
     * Indicates that an operation was attempted on a compilation context before it was started.
     * For example, calling `cancelVerifiedDex2Oat` before `startVerifiedDex2Oat`.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE = -20,

    /**
     * The compilation context is missing compiler arguments.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_CTX_MISSING_ARGS = -23,

} AVerifiedDex2Oat_Status;

/**
 * A callback function to be invoked upon successful compilation.
 *
 * @param success_cb_ctx This is the same `success_ctx` passed into
 * `AVerifiedDex2Oat_createCompilationContext`.
 * @param result_ctx An opaque context from which the following can be extracted:
 *  - The amount of CPU time, in milliseconds, spent on compilation.
 *  - The amount of wall time, in milliseconds, spent on compilation.
 *
 * Safety requirements:
 * The C-owner must guarantee that this function will not block.
 * The C-owner should not retain a copy of the `stats` pointer, it is not guaranteed
 * to be valid after the callback finishes execution.
 */
typedef void (*AVerifiedDex2Oat_OnSuccessCallback)(
        AVerifiedDex2Oat_SuccessCallbackContext* _Nonnull success_cb_ctx,
        const AVerifiedDex2Oat_SuccessResultContext* _Nonnull result_ctx);

/**
 * Gets the wall time, in milliseconds, from an opaque result context.
 *
 * This should only be used by the `AVerifiedDex2Oat_OnSuccessCallback`
 * has been called after a `AVerifiedDex2Oat_start` operation.
 *
 * @param success_result_ctx An opaque success result context pointer.
 * @param wall_time_ms This will be set to the wall time, in milliseconds,
 * taken to perform the compilation.
 *
 * @return  `AVERIFIED_DEX2OAT_SUCCESS` on success
 *   `AVERIFIED_DEX2OAT_BAD_ARGS` when `wall_time_ms` is null.
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_CompilationStats_getWallClockTimeMs(
        const AVerifiedDex2Oat_SuccessResultContext* _Nonnull success_result_ctx,
        int32_t* _Nonnull wall_time_ms) __INTRODUCED_IN(37);

/**
 * Gets the cpu time, in milliseconds, from an opaque result context.
 *
 * This should only be used by the `AVerifiedDex2Oat_OnSuccessCallback`
 * has been called after a `AVerifiedDex2Oat_start` operation.
 *
 * @param success_result_ctx An opaque success result context pointer.
 *  This must be the `success_result_ctx` passed into the
 *  `AVerifiedDex2Oat_OnSuccessCallback` function.
 * @param cpu_time_ms The integer this points to will be set to the
 * CPU time, in milliseconds, taken to perform the compilation.
 *
 * @return  `AVERIFIED_DEX2OAT_SUCCESS` on success
 *   `AVERIFIED_DEX2OAT_BAD_ARGS` when `cpu_time_ms` is null.
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_CompilationStats_getCpuClockTimeMs(
        const AVerifiedDex2Oat_SuccessResultContext* _Nonnull success_result_ctx,
        int32_t* _Nonnull cpu_time_ms) __INTRODUCED_IN(37);

/**
 * The reason why dex2oat failed.
 */
typedef enum AVerifiedDex2Oat_FailureReason : int32_t {
    /** An error occurred during the setup phase before dex2oat was invoked.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_COMPILATION_SETUP_FAILED = 0,
    /** The dex2oat process itself failed (e.g., exited with a non-zero status).
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_DEX2OAT_FAILED = 1,
    /** Failed to enable fs-verity on the output artifacts.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_FAILED_TO_ENABLE_FSVERITY = 2,
    /** The compilation timed out.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_TIMEOUT = 3,
    /** An unknown or unspecified error occurred.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_UNKNOWN,
} AVerifiedDex2Oat_FailureReason;

/**
 * A callback function to be invoked upon compilation failure.
 *
 * @param failure_cb_ctx The same failure context provided to
 * `AVerifiedDex2Oat_createCompilationContext`
 * @param results_ctx an opaque result context.
 *
 * Safety requirements:
 * The C-owner must guarantee that this function will not block.
 */
typedef void (*AVerifiedDex2Oat_OnFailureCallback)(
        AVerifiedDex2Oat_FailureCallbackContext* _Nonnull failure_cb_ctx,
        const AVerifiedDex2Oat_FailureResultContext* _Nonnull results_ctx);

/**
 * Extracts the failure code and message from the opaque results context passed
 * into a `AVerifiedDex2Oat_OnFailureCallback`.
 *
 * This should only be called from within the `AVerifiedDex2Oat_OnFailureCallback`.
 *
 * @param failure_result_ctx This is the opaque results context that is passed into the
 * `AVerifiedDex2Oat_OnFailureCallback`.
 * @param out_failure_code If the return code is `AVERIFIED_DEX2OAT_SUCCESS`
 * this is set to a `AVerifiedDex2Oat_Failure` code.
 *
 * @return `AVERIFIED_DEX2OAT_SUCCESS` on success
 *  `AVERIFIED_DEX2OAT_BAD_ARGS` when `out_failure_code` is a null pointer.
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_FailureInfo_getFailureCode(
        AVerifiedDex2Oat_FailureResultContext* _Nonnull failure_result_ctx,
        AVerifiedDex2Oat_FailureReason* _Nonnull out_failure_code) __INTRODUCED_IN(37);

/**
 * Extracts the message from the opaque results context passed
 * into a `AVerifiedDex2Oat_OnFailureCallback`.
 *
 * This should only be called from within the `AVerifiedDex2Oat_OnFailureCallback`.
 *
 * @param failure_result_ctx This is the opaque results context that is passed into the
 * `AVerifiedDex2Oat_OnFailureCallback`.
 * @param message_ptr_ptr If the return code is `AVERIFIED_DEX2OAT_SUCCESS`
 * then the pointer this pointer points to will instead point to a null
 * terminated message UTF-8 string. The string that is pointed to will live until
 * `AVerifiedDex2Oat_OnFailureCallback` exits.
 *
 * @return `AVERIFIED_DEX2OAT_SUCCESS` on success
 *  `AVERIFIED_DEX2OAT_BAD_ARGS` when `message` is a null pointer.
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_FailureInfo_getMessage(
        AVerifiedDex2Oat_FailureResultContext* _Nonnull failure_result_ctx,
        const char** _Nonnull message) __INTRODUCED_IN(37);

/**
 * Create an opaque compilation context needed for starting a dex2oat operation.
 *
 * AVerifiedDex2Oat_destroyCompilationContext must be called to free the associated memory after it
 * is no longer used.
 * AVerifiedDex2Oat_createCompilationContext will block until the composd
 * is available or until timeout_seconds elapses, whichever comes first.
 *
 * @param out_ctx On success the pointer this pointer points to will be set to a compilation
 * context. This context should only be destroyed using
 * `AVerifiedDex2Oat_destroyCompilationContext`.
 * @param on_success_cb After a compilation is started using `AVerifiedDex2Oat_start`
 * this function will be called if the compilation is successful.
 * @param success_ctx On a successful compilation this context will be passed into the callback.
 * sub-context which can be extracted using `AVerifiedDex2Oat_extractSuccessSubcontextFromContext`
 * @param on_failure_cb After a compilation is started using `AVerifiedDex2Oat_start`
 * this function will be called if the compilation fails.
 * @param failure_ctx On a failed compilation, this context will be passed into the callback.
 * @param recorded_args_fd This is the file descriptor where the compilation arguments should be
 * recorded into.
 * @param timeout_seconds The number of seconds to wait for the `composd` service before giving up.
 *
 * @return
 *   - `AVERIFIED_DEX2OAT_SUCCESS` on success
 *   - `AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE` if the service cannot be reached.
 *   - `AVERIFIED_DEX2OAT_CTX_MISSING_ARGS` if compiler arguments have not been added to
 *     compilation context.
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_createCompilationContext(
        AVerifiedDex2Oat_CompilationContext** _Nonnull out_ctx,
        AVerifiedDex2Oat_OnSuccessCallback _Nonnull on_success_cb,
        AVerifiedDex2Oat_SuccessCallbackContext* success_ctx,
        AVerifiedDex2Oat_OnFailureCallback _Nonnull on_failure_cb,
        AVerifiedDex2Oat_FailureCallbackContext* failure_ctx, int32_t recorded_args_fd,
        uint64_t timeout_seconds) __INTRODUCED_IN(37);

/**
 * Add a single dex2oat argument to the compilation context.
 *
 * @param compilation_ctx A compilation context created by
 *   `AVerifiedDex2Oat_createCompilationContext`
 * @param format_string An argument for the compiler as a UTF-8 null terminated
 *   string. An unescaped `!` in `format_string` is substituted with `fd`. `!`
 *   can be escaped with a preceding `\`.
 * @param fds A list of file descriptors used to substitute the `!` in
 *   `format_string`. The number of `!` should match the length of `fds`.
 *   Each file descriptor should:
 *   - Be opened and be `rw` if the file is meant to be written and `ro` if the
 *     file is meant to be read by the compilation operation.
 *   - Have its ownership relinquished.
 *   - Be ordered in the intended `format_string` substitution order, for example the first `!`
 *     in `format_string` will be substituted by `fd[0]`.
 *   The file descriptors will be closed when `AVerifiedDex2Oat_destroyCompilationContext`
 *   is called on `compilation_ctx`.
 * @param fd_count the number of file descriptors in `fd`.
 *
 * @return
 *  - `AVERIFIED_DEX2OAT_SUCCESS` when the argument has successfully been added to the compilation
 *    context.
 *  - `AVERIFIED_DEX2OAT_ERROR_BAD_ARGS` if compilation_ctx is null, format_string is null
 *    or any file descriptors in `fds` is invalid.
 *  - `AVERIFIED_DEX2OAT_CTX_ALREADY_STARTED` if `compilation_ctx` had `AVerifiedDex2Oat_start`
 * called on it.
 *  - `AVERIFIED_DEX2OAT_BAD_ARGS_FORMAT_STRING_NOT_UTF8` if `format_string is not a UTF-8 string.
 *  - `AVERIFIED_DEX2OAT_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS` if the number of placeholders in
 *    `format_string` is not `fd_count`
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_addArgToCompilationContext(
        AVerifiedDex2Oat_CompilationContext* _Nonnull compilation_ctx,
        const char* _Nonnull format_string, const int* fds, uint32_t fd_count) __INTRODUCED_IN(37);

/**
 * Destroys an opaque compilation context created by AVerifiedDex2Oat_createCompilationContext.
 *
 * This function takes a pointer to a compilation context and frees it.
 *
 * @param compilation_ctx A compilation context that was created using
 * `AVerifiedDex2Oat_createCompilationContext`
 *
 * If `AVerifiedDex2Oat_start` has never been called on `compilation_ctx` then this function can
 * safely be called on it.
 *
 * If AVerifiedDex2Oat_start has been called on the context then this function can safely be
 * called if any of these are true:
 *  - Either the OnSuccess or OnFailure callback within the context was called.
 *  - AVerifiedDex2Oat_cancel has been called with the context.
 *
 * @return
 * - `AVERIFIED_DEX2OAT_SUCCESS` when the context has successfully been destroyed.
 * - `AVERIFIED_DEX2OAT_ERROR_TIMED_OUT` timed out while destroying context.
 */
void AVerifiedDex2Oat_destroyCompilationContext(
        AVerifiedDex2Oat_CompilationContext* _Nonnull compilation_ctx) __INTRODUCED_IN(37);

/**
 * Starts a dex2oat compilation within a VM.
 *
 * This function starts a verified dex2oat compilation process using the provided
 * compilation context.
 *
 * The result of the compilation is communicated asynchronously
 * via the success and failure callbacks provided when the context was created.
 *
 * @param compilation_ctx A compilation context created by
 *    `AVerifiedDex2Oat_createCompilationContext` and
 *      -  has a least one compiler argument added to it using
 *        `AVerifiedDex2Oat_addArgToCompilationContext.
 *      - `AVerifiedDex2Oat_start` has never been called on it.
 *      - `AVerifiedDex2Oat_cancel` has never been called on it.
 * @param timeout_seconds The timeout for the compilation in seconds.
 *
 * @return
 *   - `AVERIFIED_DEX2OAT_SUCCESS` on success
 *   - `AVERIFIED_DEX2OAT_BAD_ARGS` when
 *     - compilation context is invalid
 *   - `AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE` if start was already called on
 *     this context.
 *   - `AVERIFIED_DEX2OAT_CTX_MISSING_ARGS` if the compilation context is missing
 *      compiler arguments.
 *   - `AVERIFIED_DEX2OAT_ERROR_CALLING_COMPOS` an error occurred when trying
 *     to start the compilation process.
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_start(
        AVerifiedDex2Oat_CompilationContext* _Nonnull compilation_ctx, uint32_t timeout_seconds)
        __INTRODUCED_IN(37);

/*
 * Cancels an started dex2oat compilation.
 *
 * This function attempts to cancel a compilation that was previously started with
 * `AVerifiedDex2Oat_start`.
 *
 * @param compilation_ctx A  compilation context that has had
 *   `AVerifiedDex2Oat_start called on it. After a successful cancel the context
 *    should be destroyed.
 *
 * @return
 *   - `SUCCESS` on successful cancellation
 *   - `AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE` no compilation in progress.
 *     Either the compilation was never started, has been canceled or has finished.
 *   - `AVERIFIED_DEX2OAT_ERROR` unable to cancel due to an unrecoverable error.
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_cancel(
        const AVerifiedDex2Oat_CompilationContext* _Nonnull compilation_ctx) __INTRODUCED_IN(37);
__END_DECLS
