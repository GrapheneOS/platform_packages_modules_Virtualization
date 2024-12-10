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

import android.app.Notification;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.Parcel;
import android.os.ResultReceiver;
import android.system.virtualmachine.VirtualMachine;
import android.system.virtualmachine.VirtualMachineConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig;
import android.system.virtualmachine.VirtualMachineCustomImageConfig.Disk;
import android.system.virtualmachine.VirtualMachineException;
import android.util.Log;
import android.widget.Toast;

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
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Objects;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class VmLauncherService extends Service implements DebianServiceImpl.DebianServiceCallback {
    private static final String EXTRA_NOTIFICATION = "EXTRA_NOTIFICATION";
    private static final String ACTION_START_VM_LAUNCHER_SERVICE =
            "android.virtualization.START_VM_LAUNCHER_SERVICE";

    public static final String ACTION_STOP_VM_LAUNCHER_SERVICE =
            "android.virtualization.STOP_VM_LAUNCHER_SERVICE";

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
    private PortNotifier mPortNotifier;

    private static Intent getMyIntent(Context context) {
        return new Intent(context.getApplicationContext(), VmLauncherService.class);
    }

    public interface VmLauncherServiceCallback {
        void onVmStart();

        void onVmStop();

        void onVmError();

        void onIpAddrAvailable(String ipAddr);
    }

    public static void run(
            Context context, VmLauncherServiceCallback callback, Notification notification) {
        Intent i = getMyIntent(context);
        if (i == null) {
            return;
        }
        ResultReceiver resultReceiver =
                new ResultReceiver(new Handler(Looper.myLooper())) {
                    @Override
                    protected void onReceiveResult(int resultCode, Bundle resultData) {
                        if (callback == null) {
                            return;
                        }
                        switch (resultCode) {
                            case RESULT_START:
                                callback.onVmStart();
                                return;
                            case RESULT_STOP:
                                callback.onVmStop();
                                return;
                            case RESULT_ERROR:
                                callback.onVmError();
                                return;
                            case RESULT_IPADDR:
                                callback.onIpAddrAvailable(resultData.getString(KEY_VM_IP_ADDR));
                                return;
                        }
                    }
                };
        i.putExtra(Intent.EXTRA_RESULT_RECEIVER, getResultReceiverForIntent(resultReceiver));
        i.putExtra(VmLauncherService.EXTRA_NOTIFICATION, notification);
        context.startForegroundService(i);
    }

    private static ResultReceiver getResultReceiverForIntent(ResultReceiver r) {
        Parcel parcel = Parcel.obtain();
        r.writeToParcel(parcel, 0);
        parcel.setDataPosition(0);
        r = ResultReceiver.CREATOR.createFromParcel(parcel);
        parcel.recycle();
        return r;
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (Objects.equals(intent.getAction(), ACTION_STOP_VM_LAUNCHER_SERVICE)) {
            // If there is no Debian service or it fails to shutdown, just stop the service.
            if (mDebianService == null || !mDebianService.shutdownDebian()) {
                stopSelf();
            }
            return START_NOT_STICKY;
        }
        if (mVirtualMachine != null) {
            Log.d(TAG, "VM instance is already started");
            return START_NOT_STICKY;
        }
        mExecutorService =
                Executors.newCachedThreadPool(new TerminalThreadFactory(getApplicationContext()));

        InstalledImage image = InstalledImage.getDefault(this);
        ConfigJson json = ConfigJson.from(this, image.getConfigPath());
        VirtualMachineConfig.Builder configBuilder = json.toConfigBuilder(this);
        VirtualMachineCustomImageConfig.Builder customImageConfigBuilder =
                json.toCustomImageConfigBuilder(this);
        if (overrideConfigIfNecessary(customImageConfigBuilder)) {
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
            throw new RuntimeException("cannot create runner", e);
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

        startForeground(this.hashCode(), notification);

        mResultReceiver.send(RESULT_START, null);

        mPortNotifier = new PortNotifier(this);
        startDebianServer();

        return START_NOT_STICKY;
    }

    private boolean overrideConfigIfNecessary(VirtualMachineCustomImageConfig.Builder builder) {
        boolean changed = false;
        // TODO: check if ANGLE is enabled for the app.
        if (Files.exists(ImageArchive.getSdcardPathForTesting().resolve("virglrenderer"))) {
            builder.setGpuConfig(
                    new VirtualMachineCustomImageConfig.GpuConfig.Builder()
                            .setBackend("virglrenderer")
                            .setRendererUseEgl(true)
                            .setRendererUseGles(true)
                            .setRendererUseGlx(false)
                            .setRendererUseSurfaceless(true)
                            .setRendererUseVulkan(false)
                            .setContextTypes(new String[] {"virgl2"})
                            .build());
            Toast.makeText(this, R.string.virgl_enabled, Toast.LENGTH_SHORT).show();
            changed = true;
        }

        InstalledImage image = InstalledImage.getDefault(this);
        if (image.hasBackup()) {
            Path backup = image.getBackupFile();
            builder.addDisk(Disk.RWDisk(backup.toString()));
            changed = true;
        }
        return changed;
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

    @Override
    public void onIpAddressAvailable(String ipAddr) {
        android.os.Trace.endAsyncSection("debianBoot", 0);
        Bundle b = new Bundle();
        b.putString(VmLauncherService.KEY_VM_IP_ADDR, ipAddr);
        mResultReceiver.send(VmLauncherService.RESULT_IPADDR, b);
    }

    public static void stop(Context context) {
        Intent i = getMyIntent(context);
        i.setAction(VmLauncherService.ACTION_STOP_VM_LAUNCHER_SERVICE);
        context.startService(i);
    }

    @Override
    public void onDestroy() {
        if (mPortNotifier != null) {
            mPortNotifier.stop();
        }
        getSystemService(NotificationManager.class).cancelAll();
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
        super.onDestroy();
    }

    private void stopDebianServer() {
        if (mDebianService != null) {
            mDebianService.killForwarderHost();
        }
        if (mServer != null) {
            mServer.shutdown();
        }
    }
}
