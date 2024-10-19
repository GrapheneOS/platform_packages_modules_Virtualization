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

import com.android.virtualization.vmlauncher.proto.DebianServiceGrpc;
import com.android.virtualization.vmlauncher.proto.Empty;
import com.android.virtualization.vmlauncher.proto.ForwardingRequestItem;
import com.android.virtualization.vmlauncher.proto.IpAddr;
import com.android.virtualization.vmlauncher.proto.ReportVmIpAddrResponse;

import io.grpc.stub.StreamObserver;

class DebianServiceImpl extends DebianServiceGrpc.DebianServiceImplBase {
    public static final String TAG = "DebianService";
    private final DebianServiceCallback mCallback;

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
            Empty request, StreamObserver<ForwardingRequestItem> responseObserver) {
        Log.d(DebianServiceImpl.TAG, "OpenForwardingRequestQueue");

        // TODO(b/340126051): Bring information from forwarder_host.

        responseObserver.onCompleted();
    }

    protected interface DebianServiceCallback {
        void onIpAddressAvailable(String ipAddr);
    }
}
