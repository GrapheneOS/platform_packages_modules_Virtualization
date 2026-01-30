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

#pragma once
#include <stdint.h>
#include <sys/cdefs.h>

__BEGIN_DECLS
/**
 * This API provides the means to perform a dex2oat compilation within the
 * CompOS protected VM. This API is meant to be used only by ARTd, use of this
 * API by anything else will result in a failure.
 */

/**
 * Contains the
 * - on success/on failure callbacks
 * - the user provided data to pass to the success/failure callbacks
 * - the arguments for compilation
 * - the file descriptor where the compilation arguments and results will be
 *   recorded.
 */
typedef struct AVerifiedDex2Oat_CompilationContext AVerifiedDex2Oat_CompilationContext;

/**
 * Contains information about a successful compilation.
 */
typedef struct AVerifiedDex2Oat_SuccessResultContext AVerifiedDex2Oat_SuccessData;

/**
 * Contains information about a failed compilation.
 */
typedef struct AVerifiedDex2Oat_FailureResultContext AVerifiedDex2Oat_FailureData;

/**
 * Represents the status codes returned by the AVerifiedDex2Oat APIs.
 * Introduced in API Level 37.
 */
typedef enum AVerifiedDex2Oat_Status : int32_t {
    /** Indicates that the `AVerifiedDex2Oat` API call completed successfully.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_SUCCESS = 0,

    /** Indicates that one or more arguments provided to the AVerifiedDex2Oat API were invalid.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_BAD_ARGS = 1,

    /**
     * Indicates that the number of file descriptor placeholders in
     * `formatString` when calling `AVerifiedDex2Oat_CompilationContext_addArg`
     * does not match the number of provided file descriptors.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS = 2,

    /** Indicates that a `Dex2OatArg` format string was not valid
     * null-terminated UTF-8 string(where null is allowed only for string
     * termination)
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_BAD_ARGS_FORMAT_STRING_NOT_UTF8 = 3,

    /** Indicates a generic error occurred when calling into the `composd` service.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_ERROR_CALLING_COMPOS = 10,

    /** Indicates that the `composd` service was not available or timed out during connection.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE = 11,

    /**
     * The operation timed out.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_ERROR_TIMED_OUT = 12,

    /**
     * A general error.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_ERROR_GENERAL = 19,

    /**
     * Indicates that an operation was attempted on a compilation context in an
     * unexpected state.
     * For example, calling `AVerifiedDex2Oat_CompilationContext_cancel` before
     * `AVerifiedDex2Oat_CompilationContext_start`.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE = 20,

    /**
     * The compilation context is missing compiler arguments.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_CTX_MISSING_ARGS = 23,

} AVerifiedDex2Oat_Status;

/**
 * @brief Converts an `AVerifiedDex2Oat_Status` enum to a string.
 *
 * @param status The `AVerifiedDex2Oat_Status` enum to be converted.
 * @return A pointer to a static, null-terminated UTF-8 string(where null is
 * allowed only for string termination) representation of the
 * status.
 */
const char* _Nonnull AVerifiedDex2Oat_Status_toString(AVerifiedDex2Oat_Status status)
        __INTRODUCED_IN(37);

/**
 * A callback function invoked after a successful compilation.
 *
 * This function may be invoked from a different thread than the
 * thread that called `AVerifiedDex2Oat_start`.
 *
 * @param successCbUserData This is the user data that was added to
 * the compilation context via `AVerifiedDex2Oat_CompilationContext_start`
 * @param resultCtx An opaque context from which the following can be extracted:
 *  - The amount of CPU time, in milliseconds, spent on compilation.
 *  - The amount of wall time, in milliseconds, spent on compilation.
 *
 * Safety requirements:
 * This function must not block.
 * The `result_ctx` pointer is valid until the callback returns.
 */
typedef void (*AVerifiedDex2Oat_onSuccessCallback)(
        const AVerifiedDex2Oat_SuccessData* _Nonnull resultCtx,
        void* _Null_unspecified successCbUserData);

/**
 * Gets the wall time, in milliseconds, from an opaque result context.
 *
 * This should only be used by the `AVerifiedDex2Oat_onSuccessCallback`
 * which has been called after a `AVerifiedDex2Oat_start` operation.
 *
 * @param successData A pointer to the success data provided
 * to the `AVerifiedDex2Oat_onSuccessCallback` callback.
 *
 * @return The wall clock time, in milliseconds, that it took to compile.
 */
uint32_t AVerifiedDex2Oat_SuccessData_getWallClockTimeMs(
        const AVerifiedDex2Oat_SuccessData* _Nonnull successData) __INTRODUCED_IN(37);

/**
 * Gets the cpu time, in milliseconds, from an opaque result context.
 *
 * This should only be used by the `AVerifiedDex2Oat_onSuccessCallback`
 * which has been called after a `AVerifiedDex2Oat_start` operation.
 *
 * @param successData A pointer to the success data provided
 * to the `AVerifiedDex2Oat_onSuccessCallback` callback.
 *
 * @return The CPU time, in milliseconds, that it took to compile.
 */
uint32_t AVerifiedDex2Oat_SuccessData_getCpuClockTimeMs(
        const AVerifiedDex2Oat_SuccessData* _Nonnull successData) __INTRODUCED_IN(37);

/**
 * The reason why dex2oat failed.
 */
typedef enum AVerifiedDex2Oat_FailureReason : int32_t {
    /** An error occurred during the setup phase before dex2oat was invoked.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_COMPILATION_SETUP_FAILED = 1,
    /** The dex2oat process itself failed (e.g., exited with a non-zero status,
     * or signaled).
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_DEX2OAT_FAILED = 2,
    /** Failed to enable fs-verity on the output artifacts.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_FAILED_TO_ENABLE_FSVERITY = 3,
    /** The compilation timed out.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_TIMEOUT = 4,
    /** An unknown or unspecified error occurred.
     * This should always be the last value in the enum.
     * New values should be inserted above.
     * Introduced in API Level 37.
     */
    AVERIFIED_DEX2OAT_FAILURE_UNKNOWN = INT32_MAX
} AVerifiedDex2Oat_FailureReason;

/**
 * @brief Converts an `AVerifiedDex2Oat_FailureReason` enum to a string.
 *
 * @param reason The `AVerifiedDex2Oat_FailureReason` enum to be converted.
 * @return A pointer to a static, null-terminated UTF-8 string(where null is
 * allowed only for string termination) representation of the reason.
 */
const char* _Nonnull AVerifiedDex2Oat_FailureReason_toString(AVerifiedDex2Oat_FailureReason reason)
        __INTRODUCED_IN(37);

/**
 * A callback function to be invoked upon compilation failure.
 *
 * No assumptions should be made about which thread this callback is called
 * from.
 *
 * @param failureData contains the specifics of why the compilation failed.
 * @param failureUserData The same failure context provided to
 * `AVerifiedDex2Oat_CompilationContext_start` Since this callback is called
 * from a different thread, access to `failureUserData` must be thread safe.
 *
 * Safety requirements:
 *  This callback must not block.
 *  `result_ctx` is invalid after the callback returns.
 */
typedef void (*AVerifiedDex2Oat_onFailureCallback)(
        const AVerifiedDex2Oat_FailureData* _Nonnull failureData,
        void* _Null_unspecified failureUserData);

/**
 * Gets the failure reason from failure data.
 *
 * Only `AVerifiedDex2Oat_onFailureCallback` should call this function.
 *
 * @param failureData Contains information about why a compilation failed.
 *
 * @return An `AVerifiedDex2Oat_FailureReason` detailing why the compilation failed.
 */
AVerifiedDex2Oat_FailureReason AVerifiedDex2Oat_FailureData_getReason(
        const AVerifiedDex2Oat_FailureData* _Nonnull failureData) __INTRODUCED_IN(37);

/**
 * Gets the exit code from failure data.
 *
 * Only `AVerifiedDex2Oat_onFailureCallback` should call this function.
 *
 * @param failureData Contains information about why a compilation failed.
 *
 * @return If the failure reason is `AVERIFIED_DEX2OAT_DEX2OAT_FAILED` and
 * `dex2oat` exited with a non zero exit code then the exit code of `dex2oat`
 * is returned. Otherwise, -1 is returned.
 */
int32_t AVerifiedDex2Oat_FailureData_getExitCode(
        const AVerifiedDex2Oat_FailureData* _Nonnull failureData) __INTRODUCED_IN(37);

/**
 * Gets the POSIX signal value that caused a compilation failure.
 *
 * Only `AVerifiedDex2Oat_onFailureCallback` should call this function.
 *
 * @param failureData Contains information about why a compilation failed.

 * @return If the failure reason is `AVERIFIED_DEX2OAT_DEX2OAT_FAILED` and
 * `dex2oat` failed due to a signal, the POSIX signal value is returned.
 *  Otherwise 0 is returned.
 */
uint32_t AVerifiedDex2Oat_FailureData_getSignal(
        const AVerifiedDex2Oat_FailureData* _Nonnull failureData) __INTRODUCED_IN(37);

/**
 * Gets the cpu time, in milliseconds, before compilation failed from failure data.
 *
 * Only `AVerifiedDex2Oat_onFailureCallback` should call this function.
 *
 * @param failureData Contains information about why a compilation failed.
 *
 * @return The amount of cpu time, in milliseconds, compilation ran for before failing.
 */
uint32_t AVerifiedDex2Oat_FailureData_getCpuClockTimeMs(
        const AVerifiedDex2Oat_FailureData* _Nonnull failureData) __INTRODUCED_IN(37);

/**
 * Gets the wall time, in milliseconds, from failure data.
 *
 * Only `AVerifiedDex2Oat_onFailureCallback` should call this function.
 *
 * @param failureData Contains information about why a compilation failed.
 *
 * @return The wall clock time, in milliseconds, spent compiling before failing.
 */
uint32_t AVerifiedDex2Oat_FailureData_getWallClockTimeMs(
        const AVerifiedDex2Oat_FailureData* _Nonnull failureData) __INTRODUCED_IN(37);

/**
 * Gets the failure message failure data.
 *
 * Only `AVerifiedDex2Oat_onFailureCallback` should call this function.
 *
 * @param failureData Contains information about why a compilation failed.
 * @return A pointer to a null terminated ASCII string (where null is only
 * allowed for termination) containing the failure message. This pointer has the
 * lifetime equal to `failureData`.
 */
const char* _Nonnull AVerifiedDex2Oat_FailureData_getMessage(
        const AVerifiedDex2Oat_FailureData* _Nonnull failureData) __INTRODUCED_IN(37);

/**
 * Create a new compilation context.
 *
 * This is a blocking function that establishes a connection to the Compilation
 * OS service. After creation at least one argument must be added to the
 * compilation context before it is used to start a compilation. It is the
 * user's responsibility to destroy the compilation context, failing to do so
 * results in a memory leak.
 *
 * @param[out] compCtx on success the pointer this pointer points to will be set to
 * the newly created compilation context.
 * @param timeoutSeconds The number of seconds to wait for the Compilation OS
 * service before timing out.
 *
 * @return
 *   - `AVERIFIED_DEX2OAT_SUCCESS` on success
 *   - `AVERIFIED_DEX2OAT_ERROR_COMPOS_SERVICE_UNAVAILABLE` if Compilation OS
 *      service can not be reached.
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_CompilationContext_create(
        AVerifiedDex2Oat_CompilationContext* _Nullable* _Nonnull compCtx, uint64_t timeoutSeconds)
        __INTRODUCED_IN(37);

/**
 * Destroys the compilation context.
 *
 * This function is non-blocking and not thread safe and must not be called more
 * than once on an `AVerifiedDex2Oat_CompilationContext`.
 * Typically the compilation context will be freed immediately but in rare cases
 * where a completion handler (`AVerifiedDex2Oat_OnSuccessCallback`,
 * `AVerifiedDex2Oat_OnFailureCallback`) is executed the destruction of the
 * compilation context may occur after the return of this function.
 *
 * @param compCtx A compilation context that was created using
 * `AVerifiedDex2Oat_createCompilationContext`
 */
void AVerifiedDex2Oat_CompilationContext_destroy(
        const AVerifiedDex2Oat_CompilationContext* _Nonnull compCtx) __INTRODUCED_IN(37);

/**
 * Add a dex2oat argument to the compilation context.
 *
 * Adds arguments that will be passed into `dex2oat`. There is no
 * need to pass the path to the `dex2oat` binary as the first argument.
 *
 * @param compCtx A compilation context created by
 *   `AVerifiedDex2Oat_createCompilationContext`
 * @param formatString An argument for the compiler as a null-terminated UTF-8
 * string(where null is allowed only for termination). An unescaped `!` in
 * `formatString` is substituted with `fd`. `!` can be escaped with a
 * preceding `\`.
 * @param fds A list of file descriptors used to substitute the `!` in
 *   `formatString`. The number of `!` should match the length of `fds`.
 *   Each file descriptor should:
 *   - Be opened and be `rw` if the file is meant to be written and `ro` if the
 *     file is meant to be read by the compilation operation.
 *   - Be ordered in the intended `formatString` substitution order, for example the first `!`
 *     in `formatString` will be substituted by `fd[0]`.
 *   Each file descriptor provided is duplicated and the caller will retain
 *   ownership of the original file descriptor.
 * @param fdCount the number of file descriptors in `fd`.
 *
 * @return
 *  - `AVERIFIED_DEX2OAT_SUCCESS` when the argument has successfully been added to the compilation
 *    context.
 *  - `AVERIFIED_DEX2OAT_BAD_ARGS` if compCtx is null, formatString is null
 *    or any file descriptors in `fds` is invalid.
 *  - `AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE` if `compCtx`
 *    had `AVerifiedDex2Oat_start` called on it.
 *  - `AVERIFIED_DEX2OAT_BAD_ARGS_FORMAT_STRING_NOT_UTF8` if `formatString is not a null-terminated
 * UTF-8 string(where null is only allowed for string termination).
 *  - `AVERIFIED_DEX2OAT_BAD_ARGS_UNEXPECTED_NUMBER_OF_FDS` if the number of placeholders in
 *    `formatString` is not `fdCount`
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_CompilationContext_addArg(
        AVerifiedDex2Oat_CompilationContext* _Nonnull compCtx, const char* _Nonnull formatString,
        const int* _Nullable fds, size_t fdCount) __INTRODUCED_IN(37);

/**
 * Starts a dex2oat compilation within a VM.
 *
 * This function is restricted for use by ARTd, use by anything else will
 * result in a failure.
 * Starts a verified dex2oat compilation process using the provided compilation
 * context. Callers should expect this function to briefly block, typically
 * for a few hundred milliseconds.
 * The result of the compilation is communicated asynchronously
 * via the success and failure callbacks provided.
 * Once this is called the caller should refrain from writing to the
 * `recordedArgsFd` or any file descriptors added as arguments to the
 * compilation context.
 *
 * @param compCtx A compilation context created by
 * `AVerifiedDex2Oat_createCompilationContext` and
 *   -  has a least one compiler argument added to it using
 *     `AVerifiedDex2Oat_addArgToCompilationContext.
 *   - `AVerifiedDex2Oat_start` has never been called on it.
 *   - `AVerifiedDex2Oat_cancel` has never been called on it.
 * @param onSuccessCb After a compilation is started using `AVerifiedDex2Oat_start`
 * this function will be called if the compilation is successful.
 * @param successUserData On a successful compilation this user data will be passed into the
 * callback. The user data must be valid until compilation has been finished or is canceled.
 * @param onFailureCb After a compilation is started using `AVerifiedDex2Oat_start`
 * this function will be called if the compilation fails.
 * @param failureUserData On a failed compilation, this user data will be passed into failure
 * callback. This user data must be valid until the compilation has finished.
 * @param recordedArgsFd This is the file descriptor where the compilation arguments should be
 * recorded into.
 * @param timeoutSeconds The timeout for the compilation in seconds.
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
        AVerifiedDex2Oat_CompilationContext* _Nonnull compCtx,
        AVerifiedDex2Oat_onSuccessCallback _Nonnull onSuccessCb, void* _Nullable successUserData,
        AVerifiedDex2Oat_onFailureCallback _Nonnull onFailureCb, void* _Nullable failureUserData,
        int32_t recordedArgsFd, uint32_t timeoutSeconds) __INTRODUCED_IN(37);

/*
 * Cancels a started dex2oat compilation.
 *
 * This function attempts to cancel a compilation that was previously started with
 * `AVerifiedDex2Oat_start`. Completion callbacks (on success callback,
 * on failure callback) will not be invoked by cancelled compilations.
 * Expect this function to briefly block while cancelling the compilation.
 *
 * @param compCtx A compilation context that has had `AVerifiedDex2Oat_start`
 * called on it. After a successful cancel the context should be destroyed.
 *
 * @return
 *   - `SUCCESS` on successful cancellation
 *   - `AVERIFIED_DEX2OAT_CTX_UNEXPECTED_COMPILATION_STATE` no compilation in progress.
 *     Either the compilation was never started, has been canceled or has finished.
 *   - `AVERIFIED_DEX2OAT_ERROR` unable to cancel due to an unrecoverable error.
 *   - `AVERIFIED_DEX2OAT_ERROR_CALLING_COMPOS` the call to CompoOS to cancel the compilation
 *     failed.
 */
AVerifiedDex2Oat_Status AVerifiedDex2Oat_cancel(
        AVerifiedDex2Oat_CompilationContext* _Nonnull compCtx) __INTRODUCED_IN(37);

__END_DECLS