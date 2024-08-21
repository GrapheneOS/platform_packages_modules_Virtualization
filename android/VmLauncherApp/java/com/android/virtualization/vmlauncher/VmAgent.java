/*
 * Copyright (C) 2024 The Android Open Source Project
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

package com.android.virtualization.vmlauncher;

import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineException;
import android.util.Log;

import libcore.io.Streams;

import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;

/**
 * Agent running in the VM. This class provides connection to the agent and ways to communicate with
 * it.
 */
class VmAgent {
    private static final String TAG = MainActivity.TAG;
    private static final int DATA_SHARING_SERVICE_PORT = 3580;
    private static final int HEADER_SIZE = 8; // size of the header
    private static final int SIZE_OFFSET = 4; // offset of the size field in the header
    private static final long RETRY_INTERVAL_MS = 1_000;

    static final byte READ_CLIPBOARD_FROM_VM = 0;
    static final byte WRITE_CLIPBOARD_TYPE_EMPTY = 1;
    static final byte WRITE_CLIPBOARD_TYPE_TEXT_PLAIN = 2;
    static final byte OPEN_URL = 3;

    private final VirtualMachine mVirtualMachine;

    VmAgent(VirtualMachine vm) {
        mVirtualMachine = vm;
    }

    /**
     * Connects to the agent and returns the established communication channel. This can block.
     *
     * @throws InterruptedException If the current thread was interrupted
     */
    Connection connect() throws InterruptedException {
        boolean shouldLog = true;
        while (true) {
            if (Thread.interrupted()) {
                throw new InterruptedException();
            }
            try {
                return new Connection(mVirtualMachine.connectVsock(DATA_SHARING_SERVICE_PORT));
            } catch (VirtualMachineException e) {
                if (shouldLog) {
                    shouldLog = false;
                    Log.d(TAG, "Still waiting for VM agent to start", e);
                }
            }
            SystemClock.sleep(RETRY_INTERVAL_MS);
        }
    }

    static class Data {
        final int type;
        final byte[] data;

        Data(int type, byte[] data) {
            this.type = type;
            this.data = data;
        }
    }

    /** Represents a connection to the agent */
    class Connection {
        private final ParcelFileDescriptor mConn;

        private Connection(ParcelFileDescriptor conn) {
            mConn = conn;
        }

        /** Send data of a given type. This can block. */
        void sendData(byte type, byte[] data) {
            // Byte 0: Data type
            // Byte 1-3: Padding alignment & Reserved for other use cases in the future
            // Byte 4-7: Data size of the payload
            ByteBuffer header = ByteBuffer.allocate(HEADER_SIZE);
            header.clear();
            header.order(ByteOrder.LITTLE_ENDIAN);
            header.put(0, type);
            int dataSize = data == null ? 0 : data.length;
            header.putInt(SIZE_OFFSET, dataSize);

            try (OutputStream out = new FileOutputStream(mConn.getFileDescriptor())) {
                out.write(header.array());
                if (data != null) {
                    out.write(data);
                }
            } catch (IOException e) {
                throw new RuntimeException("Failed to send message of type: " + type, e);
            }
        }

        /** Read data from agent. This can block. */
        Data readData() {
            ByteBuffer header = ByteBuffer.allocate(HEADER_SIZE);
            header.clear();
            header.order(ByteOrder.LITTLE_ENDIAN);
            byte[] data;

            try (InputStream in = new FileInputStream(mConn.getFileDescriptor())) {
                Streams.readFully(in, header.array());
                byte type = header.get(0);
                int dataSize = header.getInt(SIZE_OFFSET);
                data = new byte[dataSize];
                Streams.readFully(in, data);
                return new Data(type, data);
            } catch (IOException e) {
                throw new RuntimeException("Failed to read data", e);
            }
        }

        /** Convenient method for sending data and then reading response for it. This can block. */
        Data sendAndReceive(byte type, byte[] data) {
            sendData(type, data);
            return readData();
        }
    }
}
