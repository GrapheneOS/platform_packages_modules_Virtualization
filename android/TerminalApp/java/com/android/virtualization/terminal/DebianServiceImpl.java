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

import android.content.Context;
import android.content.SharedPreferences;
import android.util.Log;

import androidx.annotation.Keep;

import com.android.virtualization.terminal.proto.DebianServiceGrpc;
import com.android.virtualization.terminal.proto.ForwardingRequestItem;
import com.android.virtualization.terminal.proto.IpAddr;
import com.android.virtualization.terminal.proto.QueueOpeningRequest;
import com.android.virtualization.terminal.proto.ReportVmActivePortsRequest;
import com.android.virtualization.terminal.proto.ReportVmActivePortsResponse;
import com.android.virtualization.terminal.proto.ReportVmIpAddrResponse;

import io.grpc.stub.StreamObserver;

import java.util.Collections;
import java.util.HashSet;
import java.util.Set;

final class DebianServiceImpl extends DebianServiceGrpc.DebianServiceImplBase {
    public static final String TAG = "DebianService";
    private static final String PREFERENCE_FILE_KEY =
            "com.android.virtualization.terminal.PREFERENCE_FILE_KEY";
    private static final String PREFERENCE_FORWARDING_PORTS = "PREFERENCE_FORWARDING_PORTS";
    private static final String PREFERENCE_FORWARDING_PORT_IS_ENABLED_PREFIX =
            "PREFERENCE_FORWARDING_PORT_IS_ENABLED_";

    private final Context mContext;
    private final SharedPreferences mSharedPref;
    private SharedPreferences.OnSharedPreferenceChangeListener mPortForwardingListener;
    private final DebianServiceCallback mCallback;

    static {
        System.loadLibrary("forwarder_host_jni");
    }

    DebianServiceImpl(Context context, DebianServiceCallback callback) {
        super();
        mCallback = callback;
        mContext = context;
        mSharedPref = mContext.getSharedPreferences(PREFERENCE_FILE_KEY, Context.MODE_PRIVATE);
    }

    @Override
    public void reportVmActivePorts(
            ReportVmActivePortsRequest request,
            StreamObserver<ReportVmActivePortsResponse> responseObserver) {
        Log.d(DebianServiceImpl.TAG, "reportVmActivePorts: " + request.toString());

        SharedPreferences.Editor editor = mSharedPref.edit();
        Set<String> ports = new HashSet<>();
        for (int port : request.getPortsList()) {
            ports.add(Integer.toString(port));
            if (!mSharedPref.contains(
                    PREFERENCE_FORWARDING_PORT_IS_ENABLED_PREFIX + Integer.toString(port))) {
                editor.putBoolean(
                        PREFERENCE_FORWARDING_PORT_IS_ENABLED_PREFIX + Integer.toString(port),
                        false);
            }
        }
        editor.putStringSet(PREFERENCE_FORWARDING_PORTS, ports);
        editor.apply();

        ReportVmActivePortsResponse reply =
                ReportVmActivePortsResponse.newBuilder().setSuccess(true).build();
        responseObserver.onNext(reply);
        responseObserver.onCompleted();
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
        mPortForwardingListener =
                new SharedPreferences.OnSharedPreferenceChangeListener() {
                    @Override
                    public void onSharedPreferenceChanged(
                            SharedPreferences sharedPreferences, String key) {
                        if (key.startsWith(PREFERENCE_FORWARDING_PORT_IS_ENABLED_PREFIX)
                                || key.equals(PREFERENCE_FORWARDING_PORTS)) {
                            updateListeningPorts();
                        }
                    }
                };
        mSharedPref.registerOnSharedPreferenceChangeListener(mPortForwardingListener);
        updateListeningPorts();
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

    private static native void terminateForwarderHost();

    void killForwarderHost() {
        Log.d(DebianServiceImpl.TAG, "Stopping port forwarding");
        if (mPortForwardingListener != null) {
            mSharedPref.unregisterOnSharedPreferenceChangeListener(mPortForwardingListener);
            terminateForwarderHost();
        }
    }

    private static native void updateListeningPorts(int[] ports);

    private void updateListeningPorts() {
        updateListeningPorts(
                mSharedPref
                        .getStringSet(PREFERENCE_FORWARDING_PORTS, Collections.emptySet())
                        .stream()
                        .filter(
                                port ->
                                        mSharedPref.getBoolean(
                                                PREFERENCE_FORWARDING_PORT_IS_ENABLED_PREFIX + port,
                                                false))
                        .map(Integer::valueOf)
                        .mapToInt(Integer::intValue)
                        .toArray());
    }

    protected interface DebianServiceCallback {
        void onIpAddressAvailable(String ipAddr);
    }
}
