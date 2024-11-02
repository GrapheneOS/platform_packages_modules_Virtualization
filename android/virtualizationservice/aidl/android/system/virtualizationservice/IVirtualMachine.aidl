/*
 * Copyright 2021 The Android Open Source Project
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
package android.system.virtualizationservice;

import android.system.virtualizationservice.IVirtualMachineCallback;
import android.system.virtualizationservice.VirtualMachineState;

interface IVirtualMachine {
    /**
     * Encountered an unexpected error. This is an implementation detail and the client
     * can do nothing about it.
     * This is used as a Service Specific Exception.
     */
    const int ERROR_UNEXPECTED = -1;

    /** Get the CID allocated to the VM. */
    int getCid();

    /** Returns the current lifecycle state of the VM. */
    VirtualMachineState getState();

    /**
     * Register a Binder object to get callbacks when the state of the VM changes, such as if it
     * dies.
     */
    void registerCallback(IVirtualMachineCallback callback);

    /** Starts running the VM. */
    void start();

    /**
     * Stops this virtual machine. Stopping a virtual machine is like pulling the plug on a real
     * computer; the machine halts immediately. Software running on the virtual machine is not
     * notified with the event.
     */
    void stop();

    /** Access to the VM's memory balloon. */
    long getMemoryBalloon();
    void setMemoryBalloon(long num_bytes);

    /** Open a vsock connection to the CID of the VM on the given port. */
    ParcelFileDescriptor connectVsock(int port);

    /**
     * Create an Accessor in libbinder that will open a vsock connection
     * to the CID of the VM on the given port.
     *
     * \param instance name of the service that the accessor is responsible for.
     *        This is the same instance that we expect clients to use when trying
     *        to get the service with the ServiceManager APIs.
     *
     * \return IBinder of the IAccessor on success, or throws a service specific exception
     *         on error. See the ERROR_* values above.
     */
    IBinder createAccessorBinder(String instance, int port);

    /** Set the name of the peer end (ptsname) of the host console. */
    void setHostConsoleName(in @utf8InCpp String pathname);

    /** Suspends the VM vcpus. */
    void suspend();

    /** Resumes the suspended VM vcpus. */
    void resume();
}
