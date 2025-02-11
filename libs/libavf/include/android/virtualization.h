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
#pragma once

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/cdefs.h>
#include <time.h>

__BEGIN_DECLS

/**
 * Represents a handle on a virtual machine raw config.
 */
typedef struct AVirtualMachineRawConfig AVirtualMachineRawConfig;

/**
 * Create a new virtual machine raw config object with no properties.
 *
 * This only creates the raw config object. `name` and `kernel` must be set with
 * calls to {@link AVirtualMachineRawConfig_setName} and {@link AVirtualMachineRawConfig_setKernel}.
 * Other properties, set by {@link AVirtualMachineRawConfig_setMemoryMiB},
 * {@link AVirtualMachineRawConfig_setInitRd}, {@link AVirtualMachineRawConfig_addDisk},
 * {@link AVirtualMachineRawConfig_setProtectedVm}, and {@link AVirtualMachineRawConfig_setBalloon}
 * are optional.
 *
 * The caller takes ownership of the returned raw config object, and is responsible for creating a
 * VM by calling {@link AVirtualMachine_createRaw} or releasing it by calling
 * {@link AVirtualMachineRawConfig_destroy}.
 *
 * \return A new virtual machine raw config object. On failure (such as out of memory), it aborts.
 */
AVirtualMachineRawConfig* _Nonnull AVirtualMachineRawConfig_create(void) __INTRODUCED_IN(36);

/**
 * Destroy a virtual machine config object.
 *
 * \param config a virtual machine config object.
 *
 * `AVirtualMachineRawConfig_destroy` does nothing if `config` is null. A destroyed config object
 * must not be reused.
 */
void AVirtualMachineRawConfig_destroy(AVirtualMachineRawConfig* _Nullable config)
        __INTRODUCED_IN(36);

/**
 * Set a name of a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param name a pointer to a null-terminated, UTF-8 encoded string for the name.
 *
 * \return If successful, it returns 0. If `name` is not a null-terminated UTF-8 encoded string,
 *   it returns -EINVAL.
 */
int AVirtualMachineRawConfig_setName(AVirtualMachineRawConfig* _Nonnull config,
                                     const char* _Nonnull name) __INTRODUCED_IN(36);

/**
 * Set an instance ID of a virtual machine. Every virtual machine is identified by a unique
 * `instanceId` which the virtual machine uses as its persistent identity while performing stateful
 * operations that are expected to outlast single boot of the VM. For example, some virtual machines
 * use it as a `Id` for storing secrets in Secretkeeper, which are retrieved on next boot of th VM.
 *
 * The `instanceId` is expected to be re-used for the VM instance with an associated state (secret,
 * encrypted storage) - i.e., rebooting the VM must not change the instanceId.
 *
 * \param config a virtual machine config object.
 * \param instanceId a pointer to a 64-byte buffer for the instance ID.
 * \param instanceIdSize the number of bytes in `instanceId`.
 *
 * \return If successful, it returns 0. If `instanceIdSize` is incorrect, it returns -EINVAL.
 */
int AVirtualMachineRawConfig_setInstanceId(AVirtualMachineRawConfig* _Nonnull config,
                                           const uint8_t* _Nonnull instanceId,
                                           size_t instanceIdSize) __INTRODUCED_IN(36);

/**
 * Set a kernel image of a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param fd a readable, seekable, and sized (i.e. report a valid size using fstat()) file
 *   descriptor containing the kernel image, or -1 to unset. `AVirtualMachineRawConfig_setKernel`
 *   takes ownership of `fd`.
 */
void AVirtualMachineRawConfig_setKernel(AVirtualMachineRawConfig* _Nonnull config, int fd)
        __INTRODUCED_IN(36);

/**
 * Set an init rd of a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param fd a readable, seekable, and sized (i.e. report a valid size using fstat()) file
 *   descriptor containing the init rd image, or -1 to unset. `AVirtualMachineRawConfig_setInitRd`
 *   takes ownership of `fd`.
 */
void AVirtualMachineRawConfig_setInitRd(AVirtualMachineRawConfig* _Nonnull config, int fd)
        __INTRODUCED_IN(36);

/**
 * Add a disk for a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param fd a readable, seekable, and sized (i.e. report a valid size using fstat()) file
 *   descriptor containing the disk. `fd` must be writable if If `writable` is true.
 *   `AVirtualMachineRawConfig_addDisk` takes ownership of `fd`.
 * \param writable whether this disk should be writable by the virtual machine.
 *
 * \return If successful, it returns 0. If `fd` is invalid, it returns -EINVAL.
 */
int AVirtualMachineRawConfig_addDisk(AVirtualMachineRawConfig* _Nonnull config, int fd,
                                     bool writable) __INTRODUCED_IN(36);

/**
 * Set how much memory will be given to a virtual machine.
 *
 * When `AVirtualMachineRawConfig_setProtectedVm(..., true)` is set, the memory
 * size provided here will be automatically augmented with the swiotlb size.
 *
 * \param config a virtual machine config object.
 * \param memoryMiB the amount of RAM to give the virtual machine, in MiB. 0 or negative to use the
 *   default.
 */
void AVirtualMachineRawConfig_setMemoryMiB(AVirtualMachineRawConfig* _Nonnull config,
                                           int32_t memoryMiB) __INTRODUCED_IN(36);

/**
 * Set how much swiotlb will be given to a virtual machine.
 *
 * Only applicable when `AVirtualMachineRawConfig_setProtectedVm(..., true)` is
 * set.
 *
 * For information on swiotlb, see https://docs.kernel.org/core-api/swiotlb.html.
 *
 * \param config a virtual machine config object.
 * \param memoryMiB the amount of swiotlb to give the virtual machine, in MiB.
 *   0 or negative to use the default.
 */
void AVirtualMachineRawConfig_setSwiotlbMiB(AVirtualMachineRawConfig* _Nonnull config,
                                            int32_t swiotlbMiB) __INTRODUCED_IN(36);

/**
 * Set vCPU count. The default is 1.
 *
 * \param config a virtual machine config object.
 * \param n number of vCPUs. Must be positive.
 */
void AVirtualMachineRawConfig_setVCpuCount(AVirtualMachineRawConfig* _Nonnull config, int32_t n)
        __INTRODUCED_IN(36);

/**
 * Set whether the virtual machine's memory will be protected from the host, so the host can't
 * access its memory.
 *
 * \param config a virtual machine config object.
 * \param protectedVm whether the virtual machine should be protected.
 */
void AVirtualMachineRawConfig_setProtectedVm(AVirtualMachineRawConfig* _Nonnull config,
                                             bool protectedVm) __INTRODUCED_IN(36);

/**
 * Set whether to use an alternate, hypervisor-specific authentication method
 * for protected VMs.
 *
 * This option is discouraged. Prefer to use the default authentication method, which is better
 * tested and integrated into Android. This option must only be used from the vendor partition.
 *
 * \return If successful, it returns 0. It returns `-ENOTSUP` if the hypervisor doesn't have an
 *   alternate auth mode.
 */
int AVirtualMachineRawConfig_setHypervisorSpecificAuthMethod(
        AVirtualMachineRawConfig* _Nonnull config, bool enable) __INTRODUCED_IN(36);

/**
 * Use the specified fd as the backing memfd for a range of the guest
 * physical memory.
 *
 * \param config a virtual machine config object.
 * \param fd a memfd. Ownership is transferred, even if the function is not successful.
 * \param rangeStart range start of guest memory addresses
 * \param rangeEnd range end of guest memory addresses
 *
 * \return If successful, it returns 0. It returns `-ENOTSUP` if the hypervisor doesn't support
 *   backing memfd.
 */
int AVirtualMachineRawConfig_addCustomMemoryBackingFile(AVirtualMachineRawConfig* _Nonnull config,
                                                        int fd, uint64_t rangeStart,
                                                        uint64_t rangeEnd) __INTRODUCED_IN(36);
/**
 * Use the specified fd as the device tree overlay blob for booting VM.
 *
 * Here's the format of the device tree overlay blob.
 * link: https://source.android.com/docs/core/architecture/dto
 *
 * \param config a virtual machine config object.
 * \param fd a readable, seekable, and sized (i.e. report a valid size using fstat()) file
 *   descriptor containing device tree overlay, or -1 to unset.
 *   `AVirtualMachineRawConfig_setDeviceTreeOverlay` takes ownership of `fd`.
 */
void AVirtualMachineRawConfig_setDeviceTreeOverlay(AVirtualMachineRawConfig* _Nonnull config,
                                                   int fd) __INTRODUCED_IN(36);

/**
 * Represents a handle on a virtualization service, responsible for managing virtual machines.
 */
typedef struct AVirtualizationService AVirtualizationService;

/**
 * Spawn a new instance of `virtmgr`, a child process that will host the `VirtualizationService`
 * service, and connect to the child process.
 *
 * The caller takes ownership of the returned service object, and is responsible for releasing it
 * by calling {@link AVirtualizationService_destroy}.
 *
 * \param early set to true when running a service for early virtual machines. Early VMs are
 *   specialized virtual machines that can run even before the `/data` partition is mounted.
 *   Early VMs must be pre-defined in XML files located at `{partition}/etc/avf/early_vms*.xml`, and
 *   clients of early VMs must be pre-installed under the same partition.
 * \param service an out parameter that will be set to the service handle.
 *
 * \return
 *   - If successful, it sets `service` and returns 0.
 *   - If it fails to spawn `virtmgr`, it leaves `service` untouched and returns a negative value
 *     representing the OS error code.
 *   - If it fails to connect to the spawned `virtmgr`, it leaves `service` untouched and returns
 *     `-ECONNREFUSED`.
 */
int AVirtualizationService_create(AVirtualizationService* _Null_unspecified* _Nonnull service,
                                  bool early) __INTRODUCED_IN(36);

/**
 * Destroy a VirtualizationService object.
 *
 * `AVirtualizationService_destroy` does nothing if `service` is null. A destroyed service object
 * must not be reused.
 *
 * \param service a handle on a virtualization service.
 */
void AVirtualizationService_destroy(AVirtualizationService* _Nullable service) __INTRODUCED_IN(36);

/**
 * Represents a handle on a virtual machine.
 */
typedef struct AVirtualMachine AVirtualMachine;

/**
 * The reason why a virtual machine stopped.
 * @see AVirtualMachine_waitForStop
 */
enum AVirtualMachineStopReason : int32_t {
    /**
     * VirtualizationService died.
     */
    AVIRTUAL_MACHINE_VIRTUALIZATION_SERVICE_DIED = 1,
    /**
     * There was an error waiting for the virtual machine.
     */
    AVIRTUAL_MACHINE_INFRASTRUCTURE_ERROR = 2,
    /**
     * The virtual machine was killed.
     */
    AVIRTUAL_MACHINE_KILLED = 3,
    /**
     * The virtual machine stopped for an unknown reason.
     */
    AVIRTUAL_MACHINE_UNKNOWN = 4,
    /**
     * The virtual machine requested to shut down.
     */
    AVIRTUAL_MACHINE_SHUTDOWN = 5,
    /**
     * crosvm had an error starting the virtual machine.
     */
    AVIRTUAL_MACHINE_START_FAILED = 6,
    /**
     * The virtual machine requested to reboot, possibly as the result of a kernel panic.
     */
    AVIRTUAL_MACHINE_REBOOT = 7,
    /**
     * The virtual machine or crosvm crashed.
     */
    AVIRTUAL_MACHINE_CRASH = 8,
    /**
     * The pVM firmware failed to verify the VM because the public key doesn't match.
     */
    AVIRTUAL_MACHINE_PVM_FIRMWARE_PUBLIC_KEY_MISMATCH = 9,
    /**
     * The pVM firmware failed to verify the VM because the instance image changed.
     */
    AVIRTUAL_MACHINE_PVM_FIRMWARE_INSTANCE_IMAGE_CHANGED = 10,
    /**
     * The virtual machine was killed due to hangup.
     */
    AVIRTUAL_MACHINE_HANGUP = 11,
    /**
     * VirtualizationService sent a stop reason which was not recognised by the client library.
     */
    AVIRTUAL_MACHINE_UNRECOGNISED = 0,
};

/**
 * Create a virtual machine with given raw `config`.
 *
 * The created virtual machine is in stopped state. To run the created virtual machine, call
 * {@link AVirtualMachine_start}.
 *
 * The caller takes ownership of the returned virtual machine object, and is responsible for
 * releasing it by calling {@link AVirtualMachine_destroy}.
 *
 * \param service a handle on a virtualization service.
 * \param config a virtual machine config object. Ownership will always be transferred from the
 *   caller, even if unsuccessful. `config` must not be reused.
 * \param consoleOutFd a writable file descriptor for the console output, or -1. Ownership will
 *   always be transferred from the caller, even if unsuccessful.
 * \param consoleInFd a readable file descriptor for the console input, or -1. Ownership will always
 *   be transferred from the caller, even if unsuccessful.
 * \param logFd a writable file descriptor for the log output, or -1. Ownership will always be
 *   transferred from the caller, even if unsuccessful.
 * \param vm an out parameter that will be set to the virtual machine handle.
 *
 * \return If successful, it sets `vm` and returns 0. Otherwise, it leaves `vm` untouched and
 *   returns `-EIO`.
 */
int AVirtualMachine_createRaw(const AVirtualizationService* _Nonnull service,
                              AVirtualMachineRawConfig* _Nonnull config, int consoleOutFd,
                              int consoleInFd, int logFd,
                              AVirtualMachine* _Null_unspecified* _Nonnull vm) __INTRODUCED_IN(36);

/**
 * Start a virtual machine. `AVirtualMachine_start` is synchronous and blocks until the virtual
 * machine is initialized and free to start executing code, or until an error happens.
 *
 * \param vm a handle on a virtual machine.
 *
 * \return If successful, it returns 0. Otherwise, it returns `-EIO`.
 */
int AVirtualMachine_start(AVirtualMachine* _Nonnull vm) __INTRODUCED_IN(36);

/**
 * Stop a virtual machine. Stopping a virtual machine is like pulling the plug on a real computer;
 * the machine halts immediately. Software running on the virtual machine is not notified of the
 * event, the instance might be left in an inconsistent state.
 *
 * For a graceful shutdown, you could request the virtual machine to exit itself, and wait for the
 * virtual machine to stop by `AVirtualMachine_waitForStop`.
 *
 * A stopped virtual machine cannot be re-started.
 *
 * `AVirtualMachine_stop` stops a virtual machine by sending a signal to the process. This operation
 * is synchronous and `AVirtualMachine_stop` may block.
 *
 * \param vm a handle on a virtual machine.
 *
 * \return If successful, it returns 0. Otherwise, it returns `-EIO`.
 */
int AVirtualMachine_stop(AVirtualMachine* _Nonnull vm) __INTRODUCED_IN(36);

/**
 * Open a vsock connection to the VM on the given port. The caller takes ownership of the returned
 * file descriptor, and is responsible for closing the file descriptor.
 *
 * This operation is synchronous and `AVirtualMachine_connectVsock` may block.
 *
 * \param vm a handle on a virtual machine.
 * \param port a vsock port number.
 *
 * \return If successful, it returns a valid file descriptor. Otherwise, it returns `-EIO`.
 */
int AVirtualMachine_connectVsock(AVirtualMachine* _Nonnull vm, uint32_t port) __INTRODUCED_IN(36);

/**
 * Wait until a virtual machine stops or the given timeout elapses.
 *
 * \param vm a handle on a virtual machine.
 * \param timeout the timeout, or null to wait indefinitely.
 * \param reason An out parameter that will be set to the reason why the virtual machine stopped.
 *
 * \return
 *   - If the virtual machine stops within the timeout (or indefinitely if `timeout` is null), it
 *     sets `reason` and returns true.
 *   - If the timeout expired, it returns `false`.
 */
bool AVirtualMachine_waitForStop(AVirtualMachine* _Nonnull vm,
                                 const struct timespec* _Nullable timeout,
                                 enum AVirtualMachineStopReason* _Nonnull reason)
        __INTRODUCED_IN(36);

/**
 * Destroy a virtual machine object. If the virtual machine is still running,
 * `AVirtualMachine_destroy` first stops the virtual machine by sending a signal to the process.
 * This operation is synchronous and `AVirtualMachine_destroy` may block.
 *
 * `AVirtualMachine_destroy` does nothing if `vm` is null. A destroyed virtual machine must not be
 * reused.
 *
 * \param vm a handle on a virtual machine.
 */
void AVirtualMachine_destroy(AVirtualMachine* _Nullable vm) __INTRODUCED_IN(36);

__END_DECLS
