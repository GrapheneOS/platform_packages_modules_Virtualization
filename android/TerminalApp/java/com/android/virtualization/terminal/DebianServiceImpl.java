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

package com.android.virtualization.terminal;

import static com.android.virtualization.terminal.MainActivity.TAG;

import android.content.Context;
import android.util.Log;

import androidx.annotation.Keep;

import com.android.virtualization.terminal.proto.DebianServiceGrpc;
import com.android.virtualization.terminal.proto.ForwardingRequestItem;
import com.android.virtualization.terminal.proto.IpAddr;
import com.android.virtualization.terminal.proto.QueueOpeningRequest;
import com.android.virtualization.terminal.proto.ReportVmActivePortsRequest;
import com.android.virtualization.terminal.proto.ReportVmActivePortsResponse;
import com.android.virtualization.terminal.proto.ReportVmIpAddrResponse;
import com.android.virtualization.terminal.proto.ShutdownQueueOpeningRequest;
import com.android.virtualization.terminal.proto.ShutdownRequestItem;

import io.grpc.stub.StreamObserver;

import java.util.HashSet;
import java.util.Set;

final class DebianServiceImpl extends DebianServiceGrpc.DebianServiceImplBase {
    private final Context mContext;
    private final PortsStateManager mPortsStateManager;
    private PortsStateManager.Listener mPortsStateListener;
    private final DebianServiceCallback mCallback;
    private Runnable mShutdownRunnable;

    static {
        System.loadLibrary("forwarder_host_jni");
    }

    DebianServiceImpl(Context context, DebianServiceCallback callback) {
        super();
        mCallback = callback;
        mContext = context;
        mPortsStateManager = PortsStateManager.getInstance(mContext);
    }

    @Override
    public void reportVmActivePorts(
            ReportVmActivePortsRequest request,
            StreamObserver<ReportVmActivePortsResponse> responseObserver) {
        Log.d(TAG, "reportVmActivePorts: " + request.toString());
        mPortsStateManager.updateActivePorts(new HashSet<>(request.getPortsList()));
        ReportVmActivePortsResponse reply =
                ReportVmActivePortsResponse.newBuilder().setSuccess(true).build();
        responseObserver.onNext(reply);
        responseObserver.onCompleted();
    }

    @Override
    public void reportVmIpAddr(
            IpAddr request, StreamObserver<ReportVmIpAddrResponse> responseObserver) {
        Log.d(TAG, "reportVmIpAddr: " + request.toString());
        mCallback.onIpAddressAvailable(request.getAddr());
        ReportVmIpAddrResponse reply = ReportVmIpAddrResponse.newBuilder().setSuccess(true).build();
        responseObserver.onNext(reply);
        responseObserver.onCompleted();
    }

    @Override
    public void openForwardingRequestQueue(
            QueueOpeningRequest request, StreamObserver<ForwardingRequestItem> responseObserver) {
        Log.d(TAG, "OpenForwardingRequestQueue");
        mPortsStateListener =
                new PortsStateManager.Listener() {
                    @Override
                    public void onPortsStateUpdated(
                            Set<Integer> oldActivePorts, Set<Integer> newActivePorts) {
                        updateListeningPorts();
                    }
                };
        mPortsStateManager.registerListener(mPortsStateListener);
        updateListeningPorts();
        runForwarderHost(request.getCid(), new ForwarderHostCallback(responseObserver));
        responseObserver.onCompleted();
    }

    public boolean shutdownDebian() {
        if (mShutdownRunnable == null) {
            Log.d(TAG, "mShutdownRunnable is not ready.");
            return false;
        }
        mShutdownRunnable.run();
        return true;
    }

    @Override
    public void openShutdownRequestQueue(
            ShutdownQueueOpeningRequest request,
            StreamObserver<ShutdownRequestItem> responseObserver) {
        Log.d(TAG, "openShutdownRequestQueue");
        mShutdownRunnable =
                () -> {
                    responseObserver.onNext(ShutdownRequestItem.newBuilder().build());
                    responseObserver.onCompleted();
                    mShutdownRunnable = null;
                };
    }

    @Keep
    private static class ForwarderHostCallback {
        private StreamObserver<ForwardingRequestItem> mResponseObserver;

        ForwarderHostCallback(StreamObserver<ForwardingRequestItem> responseObserver) {
            mResponseObserver = responseObserver;
        }

        private void onForwardingRequestReceived(int guestTcpPort, int vsockPort) {
            ForwardingRequestItem item =
                    ForwardingRequestItem.newBuilder()
                            .setGuestTcpPort(guestTcpPort)
                            .setVsockPort(vsockPort)
                            .build();
            mResponseObserver.onNext(item);
        }
    }

    private static native void runForwarderHost(int cid, ForwarderHostCallback callback);

    private static native void terminateForwarderHost();

    void killForwarderHost() {
        Log.d(TAG, "Stopping port forwarding");
        if (mPortsStateListener != null) {
            mPortsStateManager.unregisterListener(mPortsStateListener);
            mPortsStateListener = null;
        }
        terminateForwarderHost();
    }

    private static native void updateListeningPorts(int[] ports);

    private void updateListeningPorts() {
        Set<Integer> activePorts = mPortsStateManager.getActivePorts();
        Set<Integer> enabledPorts = mPortsStateManager.getEnabledPorts();
        updateListeningPorts(
                activePorts.stream()
                        .filter(port -> enabledPorts.contains(port))
                        .mapToInt(Integer::intValue)
                        .toArray());
    }

    protected interface DebianServiceCallback {
        void onIpAddressAvailable(String ipAddr);
    }
}
