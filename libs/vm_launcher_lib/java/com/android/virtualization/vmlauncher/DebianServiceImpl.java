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

import android.util.Log;

import androidx.annotation.Keep;

import com.android.virtualization.vmlauncher.proto.DebianServiceGrpc;
import com.android.virtualization.vmlauncher.proto.ForwardingRequestItem;
import com.android.virtualization.vmlauncher.proto.IpAddr;
import com.android.virtualization.vmlauncher.proto.QueueOpeningRequest;
import com.android.virtualization.vmlauncher.proto.ReportVmIpAddrResponse;

import io.grpc.stub.StreamObserver;

class DebianServiceImpl extends DebianServiceGrpc.DebianServiceImplBase {
    public static final String TAG = "DebianService";
    private final DebianServiceCallback mCallback;

    static {
        System.loadLibrary("forwarder_host_jni");
    }

    protected DebianServiceImpl(DebianServiceCallback callback) {
        super();
        mCallback = callback;
    }

    @Override
    public void reportVmIpAddr(
            IpAddr request, StreamObserver<ReportVmIpAddrResponse> responseObserver) {
        Log.d(DebianServiceImpl.TAG, "reportVmIpAddr: " + request.toString());
        mCallback.onIpAddressAvailable(request.getAddr());
        ReportVmIpAddrResponse reply = ReportVmIpAddrResponse.newBuilder().setSuccess(true).build();
        responseObserver.onNext(reply);
        responseObserver.onCompleted();
    }

    @Override
    public void openForwardingRequestQueue(
            QueueOpeningRequest request, StreamObserver<ForwardingRequestItem> responseObserver) {
        Log.d(DebianServiceImpl.TAG, "OpenForwardingRequestQueue");
        runForwarderHost(request.getCid(), new ForwarderHostCallback(responseObserver));
        responseObserver.onCompleted();
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

    protected interface DebianServiceCallback {
        void onIpAddressAvailable(String ipAddr);
    }
}
