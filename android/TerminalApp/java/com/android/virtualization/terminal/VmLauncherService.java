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

import android.app.Notification;
import android.app.Service;
import android.content.Intent;
import android.os.Bundle;
import android.os.IBinder;
import android.os.ResultReceiver;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig.Disk;
import android.system.virtualmachine.VirtualMachineException;
import android.util.Log;

import io.grpc.Grpc;
import io.grpc.InsecureServerCredentials;
import io.grpc.Metadata;
import io.grpc.Server;
import io.grpc.ServerCall;
import io.grpc.ServerCallHandler;
import io.grpc.ServerInterceptor;
import io.grpc.Status;
import io.grpc.okhttp.OkHttpServerBuilder;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.nio.file.Path;
import java.util.Objects;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class VmLauncherService extends Service implements DebianServiceImpl.DebianServiceCallback {
    public static final String EXTRA_NOTIFICATION = "EXTRA_NOTIFICATION";
    private static final String TAG = "VmLauncherService";

    private static final int RESULT_START = 0;
    private static final int RESULT_STOP = 1;
    private static final int RESULT_ERROR = 2;
    private static final int RESULT_IPADDR = 3;
    private static final String KEY_VM_IP_ADDR = "ip_addr";

    private ExecutorService mExecutorService;
    private VirtualMachine mVirtualMachine;
    private ResultReceiver mResultReceiver;
    private Server mServer;
    private DebianServiceImpl mDebianService;

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    private void startForeground(Notification notification) {
        startForeground(this.hashCode(), notification);
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (Objects.equals(
                intent.getAction(), VmLauncherServices.ACTION_STOP_VM_LAUNCHER_SERVICE)) {
            stopSelf();
            return START_NOT_STICKY;
        }
        if (mVirtualMachine != null) {
            Log.d(TAG, "VM instance is already started");
            return START_NOT_STICKY;
        }
        mExecutorService = Executors.newCachedThreadPool();

        ConfigJson json = ConfigJson.from(InstallUtils.getVmConfigPath(this));
        VirtualMachineConfig.Builder configBuilder = json.toConfigBuilder(this);
        VirtualMachineCustomImageConfig.Builder customImageConfigBuilder =
                json.toCustomImageConfigBuilder(this);
        File backupFile = InstallUtils.getBackupFile(this);
        if (backupFile.exists()) {
            customImageConfigBuilder.addDisk(Disk.RWDisk(backupFile.getPath()));
            configBuilder.setCustomImageConfig(customImageConfigBuilder.build());
        }
        VirtualMachineConfig config = configBuilder.build();

        Runner runner;
        try {
            android.os.Trace.beginSection("vmCreate");
            runner = Runner.create(this, config);
            android.os.Trace.endSection();
            android.os.Trace.beginAsyncSection("debianBoot", 0);
        } catch (VirtualMachineException e) {
            Log.e(TAG, "cannot create runner", e);
            stopSelf();
            return START_NOT_STICKY;
        }
        mVirtualMachine = runner.getVm();
        mResultReceiver =
                intent.getParcelableExtra(Intent.EXTRA_RESULT_RECEIVER, ResultReceiver.class);

        runner.getExitStatus()
                .thenAcceptAsync(
                        success -> {
                            if (mResultReceiver != null) {
                                mResultReceiver.send(success ? RESULT_STOP : RESULT_ERROR, null);
                            }
                            stopSelf();
                        });
        Path logPath = getFileStreamPath(mVirtualMachine.getName() + ".log").toPath();
        Logger.setup(mVirtualMachine, logPath, mExecutorService);

        Notification notification =
                intent.getParcelableExtra(EXTRA_NOTIFICATION, Notification.class);

        startForeground(notification);

        mResultReceiver.send(RESULT_START, null);

        startDebianServer();

        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        super.onDestroy();
        stopDebianServer();
        if (mVirtualMachine != null) {
            if (mVirtualMachine.getStatus() == VirtualMachine.STATUS_RUNNING) {
                try {
                    mVirtualMachine.stop();
                    stopForeground(STOP_FOREGROUND_REMOVE);
                } catch (VirtualMachineException e) {
                    Log.e(TAG, "failed to stop a VM instance", e);
                }
            }
            mExecutorService.shutdownNow();
            mExecutorService = null;
            mVirtualMachine = null;
        }
    }

    private void startDebianServer() {
        ServerInterceptor interceptor =
                new ServerInterceptor() {
                    @Override
                    public <ReqT, RespT> ServerCall.Listener<ReqT> interceptCall(
                            ServerCall<ReqT, RespT> call,
                            Metadata headers,
                            ServerCallHandler<ReqT, RespT> next) {
                        // Refer to VirtualizationSystemService.TetheringService
                        final String VM_STATIC_IP_ADDR = "192.168.0.2";
                        InetSocketAddress remoteAddr =
                                (InetSocketAddress)
                                        call.getAttributes().get(Grpc.TRANSPORT_ATTR_REMOTE_ADDR);

                        if (remoteAddr != null
                                && Objects.equals(
                                        remoteAddr.getAddress().getHostAddress(),
                                        VM_STATIC_IP_ADDR)) {
                            // Allow the request only if it is from VM
                            return next.startCall(call, headers);
                        }
                        Log.d(TAG, "blocked grpc request from " + remoteAddr);
                        call.close(Status.Code.PERMISSION_DENIED.toStatus(), new Metadata());
                        return new ServerCall.Listener<ReqT>() {};
                    }
                };
        try {
            // TODO(b/372666638): gRPC for java doesn't support vsock for now.
            int port = 0;
            mDebianService = new DebianServiceImpl(this, this);
            mServer =
                    OkHttpServerBuilder.forPort(port, InsecureServerCredentials.create())
                            .intercept(interceptor)
                            .addService(mDebianService)
                            .build()
                            .start();
        } catch (IOException e) {
            Log.d(TAG, "grpc server error", e);
            return;
        }

        mExecutorService.execute(
                () -> {
                    // TODO(b/373533555): we can use mDNS for that.
                    String debianServicePortFileName = "debian_service_port";
                    File debianServicePortFile = new File(getFilesDir(), debianServicePortFileName);
                    try (FileOutputStream writer = new FileOutputStream(debianServicePortFile)) {
                        writer.write(String.valueOf(mServer.getPort()).getBytes());
                    } catch (IOException e) {
                        Log.d(TAG, "cannot write grpc port number", e);
                    }
                });
    }

    private void stopDebianServer() {
        if (mDebianService != null) {
            mDebianService.killForwarderHost();
        }
        if (mServer != null) {
            mServer.shutdown();
        }
    }

    @Override
    public void onIpAddressAvailable(String ipAddr) {
        android.os.Trace.endAsyncSection("debianBoot", 0);
        Bundle b = new Bundle();
        b.putString(VmLauncherService.KEY_VM_IP_ADDR, ipAddr);
        mResultReceiver.send(VmLauncherService.RESULT_IPADDR, b);
    }
}
