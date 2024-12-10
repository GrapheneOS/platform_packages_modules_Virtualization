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
 * Other properties, set by {@link AVirtualMachineRawConfig_setMemoryMib},
 * {@link AVirtualMachineRawConfig_setInitRd}, {@link AVirtualMachineRawConfig_addDisk},
 * {@link AVirtualMachineRawConfig_setProtectedVm}, and {@link AVirtualMachineRawConfig_setBalloon}
 * are optional.
 *
 * The caller takes ownership of the returned raw config object, and is responsible for creating a
 * VM by calling {@link AVirtualMachine_createRaw} or releasing it by calling
 * {@link AVirtualMachineRawConfig_destroy}.
 *
 * \return A new virtual machine raw config object.
 */
AVirtualMachineRawConfig* AVirtualMachineRawConfig_create();

/**
 * Destroy a virtual machine config object.
 *
 * \param config a virtual machine config object.
 *
 * `AVirtualMachineRawConfig_destroy` does nothing if `config` is null. A destroyed config object
 * must not be reused.
 */
void AVirtualMachineRawConfig_destroy(AVirtualMachineRawConfig* config);

/**
 * Set a name of a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param name a pointer to a null-terminated string for the name.
 *
 * \return If successful, it returns 0.
 */
int AVirtualMachineRawConfig_setName(AVirtualMachineRawConfig* config, const char* name);

/**
 * Set an instance ID of a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param instanceId a pointer to a 64-byte buffer for the instance ID.
 *
 * \return If successful, it returns 0.
 */
int AVirtualMachineRawConfig_setInstanceId(AVirtualMachineRawConfig* config,
                                           const int8_t* instanceId);

/**
 * Set a kernel image of a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param fd a readable file descriptor containing the kernel image, or -1 to unset.
 *   `AVirtualMachineRawConfig_setKernel` takes ownership of `fd`.
 *
 * \return If successful, it returns 0.
 */
int AVirtualMachineRawConfig_setKernel(AVirtualMachineRawConfig* config, int fd);

/**
 * Set an init rd of a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param fd a readable file descriptor containing the kernel image, or -1 to unset.
 *   `AVirtualMachineRawConfig_setInitRd` takes ownership of `fd`.
 *
 * \return If successful, it returns 0.
 */
int AVirtualMachineRawConfig_setInitRd(AVirtualMachineRawConfig* config, int fd);

/**
 * Add a disk for a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param fd a readable file descriptor containing the disk image.
 * `AVirtualMachineRawConfig_addDisk` takes ownership of `fd`.
 *
 * \return If successful, it returns 0. If `fd` is invalid, it returns -EINVAL.
 */
int AVirtualMachineRawConfig_addDisk(AVirtualMachineRawConfig* config, int fd);

/**
 * Set how much memory will be given to a virtual machine.
 *
 * \param config a virtual machine config object.
 * \param memoryMib the amount of RAM to give the virtual machine, in MiB. 0 or negative to use the
 *   default.
 *
 * \return If successful, it returns 0.
 */
int AVirtualMachineRawConfig_setMemoryMib(AVirtualMachineRawConfig* config, int32_t memoryMib);

/**
 * Set whether a virtual machine is protected or not.
 *
 * \param config a virtual machine config object.
 * \param protectedVm whether the virtual machine should be protected.
 *
 * \return If successful, it returns 0.
 */
int AVirtualMachineRawConfig_setProtectedVm(AVirtualMachineRawConfig* config, bool protectedVm);

/**
 * Set whether a virtual machine uses memory ballooning or not.
 *
 * \param config a virtual machine config object.
 * \param balloon whether the virtual machine should use memory ballooning.
 *
 * \return If successful, it returns 0.
 */
int AVirtualMachineRawConfig_setBalloon(AVirtualMachineRawConfig* config, bool balloon);

/**
 * Set whether to use an alternate, hypervisor-specific authentication method
 * for protected VMs. You don't want to use this.
 *
 * \return If successful, it returns 0. It returns `-ENOTSUP` if the hypervisor doesn't have an
 *   alternate auth mode.
 */
int AVirtualMachineRawConfig_setHypervisorSpecificAuthMethod(AVirtualMachineRawConfig* config,
                                                             bool enable);

/**
 * Use the specified fd as the backing memfd for a range of the guest
 * physical memory.
 *
 * \param config a virtual machine config object.
 * \param fd a memfd
 * \param rangeStart range start IPA
 * \param rangeEnd range end IPA
 *
 * \return If successful, it returns 0. It returns `-ENOTSUP` if the hypervisor doesn't support
 *   backing memfd.
 */
int AVirtualMachineRawConfig_addCustomMemoryBackingFile(AVirtualMachineRawConfig* config, int fd,
                                                        size_t rangeStart, size_t rangeEnd);

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
 * \param early set to true when running a service for early virtual machines. See
 *   [`early_vm.md`](../../../../docs/early_vm.md) for more details on early virtual machines.
 * \param service an out parameter that will be set to the service handle.
 *
 * \return
 *   - If successful, it sets `service` and returns 0.
 *   - If it fails to spawn `virtmgr`, it leaves `service` untouched and returns a negative value
 *     representing the OS error code.
 *   - If it fails to connect to the spawned `virtmgr`, it leaves `service` untouched and returns
 *     `-ECONNREFUSED`.
 */
int AVirtualizationService_create(AVirtualizationService** service, bool early);

/**
 * Destroy a VirtualizationService object.
 *
 * `AVirtualizationService_destroy` does nothing if `service` is null. A destroyed service object
 * must not be reused.
 *
 * \param service a handle on a virtualization service.
 */
void AVirtualizationService_destroy(AVirtualizationService* service);

/**
 * Represents a handle on a virtual machine.
 */
typedef struct AVirtualMachine AVirtualMachine;

/**
 * The reason why a virtual machine stopped.
 * @see AVirtualMachine_waitForStop
 */
enum StopReason : int32_t {
    /**
     * VirtualizationService died.
     */
    VIRTUALIZATION_SERVICE_DIED = 1,
    /**
     * There was an error waiting for the virtual machine.
     */
    INFRASTRUCTURE_ERROR = 2,
    /**
     * The virtual machine was killed.
     */
    KILLED = 3,
    /**
     * The virtual machine stopped for an unknown reason.
     */
    UNKNOWN = 4,
    /**
     * The virtual machine requested to shut down.
     */
    SHUTDOWN = 5,
    /**
     * crosvm had an error starting the virtual machine.
     */
    START_FAILED = 6,
    /**
     * The virtual machine requested to reboot, possibly as the result of a kernel panic.
     */
    REBOOT = 7,
    /**
     * The virtual machine or crosvm crashed.
     */
    CRASH = 8,
    /**
     * The pVM firmware failed to verify the VM because the public key doesn't match.
     */
    PVM_FIRMWARE_PUBLIC_KEY_MISMATCH = 9,
    /**
     * The pVM firmware failed to verify the VM because the instance image changed.
     */
    PVM_FIRMWARE_INSTANCE_IMAGE_CHANGED = 10,
    /**
     * The virtual machine was killed due to hangup.
     */
    HANGUP = 11,
    /**
     * VirtualizationService sent a stop reason which was not recognised by the client library.
     */
    UNRECOGNISED = 0,
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
int AVirtualMachine_createRaw(const AVirtualizationService* service,
                              AVirtualMachineRawConfig* config, int consoleOutFd, int consoleInFd,
                              int logFd, AVirtualMachine** vm);

/**
 * Start a virtual machine.
 *
 * \param vm a handle on a virtual machine.
 *
 * \return If successful, it returns 0. Otherwise, it returns `-EIO`.
 */
int AVirtualMachine_start(AVirtualMachine* vm);

/**
 * Stop a virtual machine.
 *
 * \param vm a handle on a virtual machine.
 *
 * \return If successful, it returns 0. Otherwise, it returns `-EIO`.
 */
int AVirtualMachine_stop(AVirtualMachine* vm);

/**
 * Wait until a virtual machine stops.
 *
 * \param vm a handle on a virtual machine.
 *
 * \return The reason why the virtual machine stopped.
 */
enum StopReason AVirtualMachine_waitForStop(AVirtualMachine* vm);

/**
 * Destroy a virtual machine.
 *
 * `AVirtualMachine_destroy` does nothing if `vm` is null. A destroyed virtual machine must not be
 * reused.
 *
 * \param vm a handle on a virtual machine.
 */
void AVirtualMachine_destroy(AVirtualMachine* vm);

__END_DECLS
