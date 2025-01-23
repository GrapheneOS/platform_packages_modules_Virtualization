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
package com.android.virtualization.terminal

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Parcel
import android.os.Parcelable
import android.os.ResultReceiver
import android.os.Trace
import android.system.virtualmachine.VirtualMachine
import android.system.virtualmachine.VirtualMachineCustomImageConfig
import android.system.virtualmachine.VirtualMachineException
import android.util.Log
import android.widget.Toast
import com.android.system.virtualmachine.flags.Flags.terminalGuiSupport
import com.android.virtualization.terminal.MainActivity.Companion.TAG
import com.android.virtualization.terminal.Runner.Companion.create
import com.android.virtualization.terminal.VmLauncherService.VmLauncherServiceCallback
import io.grpc.Grpc
import io.grpc.InsecureServerCredentials
import io.grpc.Metadata
import io.grpc.Server
import io.grpc.ServerCall
import io.grpc.ServerCallHandler
import io.grpc.ServerInterceptor
import io.grpc.Status
import io.grpc.okhttp.OkHttpServerBuilder
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.lang.RuntimeException
import java.net.InetSocketAddress
import java.net.SocketAddress
import java.nio.file.Files
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

class VmLauncherService : Service() {
    // TODO: using lateinit for some fields to avoid null
    private var executorService: ExecutorService? = null
    private var virtualMachine: VirtualMachine? = null
    private var resultReceiver: ResultReceiver? = null
    private var server: Server? = null
    private var debianService: DebianServiceImpl? = null
    private var portNotifier: PortNotifier? = null

    interface VmLauncherServiceCallback {
        fun onVmStart()

        fun onVmStop()

        fun onVmError()
    }

    override fun onBind(intent: Intent?): IBinder? {
        return null
    }

    override fun onStartCommand(intent: Intent, flags: Int, startId: Int): Int {
        if (intent.action == ACTION_STOP_VM_LAUNCHER_SERVICE) {
            if (debianService != null && debianService!!.shutdownDebian()) {
                // During shutdown, change the notification content to indicate that it's closing
                val notification = createNotificationForTerminalClose()
                getSystemService<NotificationManager?>(NotificationManager::class.java)
                    .notify(this.hashCode(), notification)
            } else {
                // If there is no Debian service or it fails to shutdown, just stop the service.
                stopSelf()
            }
            return START_NOT_STICKY
        }
        if (virtualMachine != null) {
            Log.d(TAG, "VM instance is already started")
            return START_NOT_STICKY
        }
        executorService = Executors.newCachedThreadPool(TerminalThreadFactory(applicationContext))

        val image = InstalledImage.getDefault(this)
        val json = ConfigJson.from(this, image.configPath)
        val configBuilder = json.toConfigBuilder(this)
        val customImageConfigBuilder = json.toCustomImageConfigBuilder(this)
        val displaySize = intent.getParcelableExtra(EXTRA_DISPLAY_INFO, DisplayInfo::class.java)
        if (overrideConfigIfNecessary(customImageConfigBuilder, displaySize)) {
            configBuilder.setCustomImageConfig(customImageConfigBuilder.build())
        }
        val config = configBuilder.build()

        Trace.beginSection("vmCreate")
        val runner: Runner =
            try {
                create(this, config)
            } catch (e: VirtualMachineException) {
                throw RuntimeException("cannot create runner", e)
            }
        Trace.endSection()
        Trace.beginAsyncSection("debianBoot", 0)

        virtualMachine = runner.vm
        resultReceiver =
            intent.getParcelableExtra<ResultReceiver?>(
                Intent.EXTRA_RESULT_RECEIVER,
                ResultReceiver::class.java,
            )

        runner.exitStatus.thenAcceptAsync { success: Boolean ->
            resultReceiver?.send(if (success) RESULT_STOP else RESULT_ERROR, null)
            stopSelf()
        }
        val logPath = getFileStreamPath(virtualMachine!!.name + ".log").toPath()
        Logger.setup(virtualMachine!!, logPath, executorService!!)

        val notification =
            intent.getParcelableExtra<Notification?>(EXTRA_NOTIFICATION, Notification::class.java)

        startForeground(this.hashCode(), notification)

        resultReceiver!!.send(RESULT_START, null)

        portNotifier = PortNotifier(this)

        // TODO: dedup this part
        val nsdManager = getSystemService<NsdManager?>(NsdManager::class.java)
        val info = NsdServiceInfo()
        info.serviceType = "_http._tcp"
        info.serviceName = "ttyd"
        nsdManager.registerServiceInfoCallback(
            info,
            executorService!!,
            object : NsdManager.ServiceInfoCallback {
                var started: Boolean = false

                override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {}

                override fun onServiceInfoCallbackUnregistered() {}

                override fun onServiceLost() {}

                override fun onServiceUpdated(info: NsdServiceInfo) {
                    Log.i(TAG, "Service found: $info")
                    if (!started) {
                        started = true
                        nsdManager.unregisterServiceInfoCallback(this)
                        startDebianServer(info.hostAddresses[0].hostAddress)
                    }
                }
            },
        )

        return START_NOT_STICKY
    }

    private fun createNotificationForTerminalClose(): Notification {
        val stopIntent = Intent()
        stopIntent.setClass(this, VmLauncherService::class.java)
        stopIntent.setAction(ACTION_STOP_VM_LAUNCHER_SERVICE)
        val stopPendingIntent =
            PendingIntent.getService(
                this,
                0,
                stopIntent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val icon = Icon.createWithResource(resources, R.drawable.ic_launcher_foreground)
        val stopActionText: String? =
            resources.getString(R.string.service_notification_force_quit_action)
        val stopNotificationTitle: String? =
            resources.getString(R.string.service_notification_close_title)
        return Notification.Builder(this, Application.CHANNEL_SYSTEM_EVENTS_ID)
            .setSmallIcon(R.drawable.ic_launcher_foreground)
            .setContentTitle(stopNotificationTitle)
            .setOngoing(true)
            .setSilent(true)
            .addAction(Notification.Action.Builder(icon, stopActionText, stopPendingIntent).build())
            .build()
    }

    private fun overrideConfigIfNecessary(
        builder: VirtualMachineCustomImageConfig.Builder,
        displayInfo: DisplayInfo?,
    ): Boolean {
        var changed = false
        // TODO: check if ANGLE is enabled for the app.
        if (Files.exists(ImageArchive.getSdcardPathForTesting().resolve("virglrenderer"))) {
            builder.setGpuConfig(
                VirtualMachineCustomImageConfig.GpuConfig.Builder()
                    .setBackend("virglrenderer")
                    .setRendererUseEgl(true)
                    .setRendererUseGles(true)
                    .setRendererUseGlx(false)
                    .setRendererUseSurfaceless(true)
                    .setRendererUseVulkan(false)
                    .setContextTypes(arrayOf<String>("virgl2"))
                    .build()
            )
            Toast.makeText(this, R.string.virgl_enabled, Toast.LENGTH_SHORT).show()
            changed = true
        }

        // Set the initial display size
        // TODO(jeongik): set up the display size on demand
        if (terminalGuiSupport() && displayInfo != null) {
            builder
                .setDisplayConfig(
                    VirtualMachineCustomImageConfig.DisplayConfig.Builder()
                        .setWidth(displayInfo.width)
                        .setHeight(displayInfo.height)
                        .setHorizontalDpi(displayInfo.dpi)
                        .setVerticalDpi(displayInfo.dpi)
                        .setRefreshRate(displayInfo.refreshRate)
                        .build()
                )
                .useKeyboard(true)
                .useMouse(true)
                .useTouch(true)
            changed = true
        }

        val image = InstalledImage.getDefault(this)
        if (image.hasBackup()) {
            val backup = image.backupFile
            builder.addDisk(VirtualMachineCustomImageConfig.Disk.RWDisk(backup.toString()))
            changed = true
        }
        return changed
    }

    private fun startDebianServer(ipAddress: String?) {
        val interceptor: ServerInterceptor =
            object : ServerInterceptor {
                override fun <ReqT, RespT> interceptCall(
                    call: ServerCall<ReqT?, RespT?>,
                    headers: Metadata?,
                    next: ServerCallHandler<ReqT?, RespT?>,
                ): ServerCall.Listener<ReqT?>? {
                    val remoteAddr =
                        call.attributes.get<SocketAddress?>(Grpc.TRANSPORT_ATTR_REMOTE_ADDR)
                            as InetSocketAddress?

                    if (remoteAddr?.address?.hostAddress == ipAddress) {
                        // Allow the request only if it is from VM
                        return next.startCall(call, headers)
                    }
                    Log.d(TAG, "blocked grpc request from $remoteAddr")
                    call.close(Status.Code.PERMISSION_DENIED.toStatus(), Metadata())
                    return object : ServerCall.Listener<ReqT?>() {}
                }
            }
        try {
            // TODO(b/372666638): gRPC for java doesn't support vsock for now.
            val port = 0
            debianService = DebianServiceImpl(this)
            server =
                OkHttpServerBuilder.forPort(port, InsecureServerCredentials.create())
                    .intercept(interceptor)
                    .addService(debianService)
                    .build()
                    .start()
        } catch (e: IOException) {
            Log.d(TAG, "grpc server error", e)
            return
        }

        executorService!!.execute(
            Runnable {
                // TODO(b/373533555): we can use mDNS for that.
                val debianServicePortFile = File(filesDir, "debian_service_port")
                try {
                    FileOutputStream(debianServicePortFile).use { writer ->
                        writer.write(server!!.port.toString().toByteArray())
                    }
                } catch (e: IOException) {
                    Log.d(TAG, "cannot write grpc port number", e)
                }
            }
        )
    }

    override fun onDestroy() {
        portNotifier?.stop()
        getSystemService<NotificationManager?>(NotificationManager::class.java).cancelAll()
        stopDebianServer()
        if (virtualMachine != null) {
            if (virtualMachine!!.getStatus() == VirtualMachine.STATUS_RUNNING) {
                try {
                    virtualMachine!!.stop()
                    stopForeground(STOP_FOREGROUND_REMOVE)
                } catch (e: VirtualMachineException) {
                    Log.e(TAG, "failed to stop a VM instance", e)
                }
            }
            executorService?.shutdownNow()
            executorService = null
            virtualMachine = null
        }
        super.onDestroy()
    }

    private fun stopDebianServer() {
        debianService?.killForwarderHost()
        server?.shutdown()
    }

    companion object {
        private const val EXTRA_NOTIFICATION = "EXTRA_NOTIFICATION"
        private const val ACTION_START_VM_LAUNCHER_SERVICE =
            "android.virtualization.START_VM_LAUNCHER_SERVICE"
        const val EXTRA_DISPLAY_INFO = "EXTRA_DISPLAY_INFO"
        const val ACTION_STOP_VM_LAUNCHER_SERVICE: String =
            "android.virtualization.STOP_VM_LAUNCHER_SERVICE"

        private const val RESULT_START = 0
        private const val RESULT_STOP = 1
        private const val RESULT_ERROR = 2

        private fun getMyIntent(context: Context): Intent {
            return Intent(context.getApplicationContext(), VmLauncherService::class.java)
        }

        fun run(
            context: Context,
            callback: VmLauncherServiceCallback?,
            notification: Notification?,
            displayInfo: DisplayInfo,
        ) {
            val i = getMyIntent(context)
            val resultReceiver: ResultReceiver =
                object : ResultReceiver(Handler(Looper.myLooper()!!)) {
                    override fun onReceiveResult(resultCode: Int, resultData: Bundle?) {
                        if (callback == null) {
                            return
                        }
                        when (resultCode) {
                            RESULT_START -> callback.onVmStart()
                            RESULT_STOP -> callback.onVmStop()
                            RESULT_ERROR -> callback.onVmError()
                        }
                    }
                }
            i.putExtra(Intent.EXTRA_RESULT_RECEIVER, getResultReceiverForIntent(resultReceiver))
            i.putExtra(EXTRA_NOTIFICATION, notification)
            i.putExtra(EXTRA_DISPLAY_INFO, displayInfo)
            context.startForegroundService(i)
        }

        private fun getResultReceiverForIntent(r: ResultReceiver): ResultReceiver {
            val parcel = Parcel.obtain()
            r.writeToParcel(parcel, 0)
            parcel.setDataPosition(0)
            return ResultReceiver.CREATOR.createFromParcel(parcel).also { parcel.recycle() }
        }

        fun stop(context: Context) {
            val i = getMyIntent(context)
            i.setAction(ACTION_STOP_VM_LAUNCHER_SERVICE)
            context.startService(i)
        }
    }
}

data class DisplayInfo(val width: Int, val height: Int, val dpi: Int, val refreshRate: Int) :
    Parcelable {
    constructor(
        source: Parcel
    ) : this(source.readInt(), source.readInt(), source.readInt(), source.readInt())

    override fun describeContents(): Int = 0

    override fun writeToParcel(dest: Parcel, flags: Int) {
        dest.writeInt(width)
        dest.writeInt(height)
        dest.writeInt(dpi)
        dest.writeInt(refreshRate)
    }

    companion object {
        @JvmField
        val CREATOR =
            object : Parcelable.Creator<DisplayInfo> {
                override fun createFromParcel(source: Parcel): DisplayInfo = DisplayInfo(source)

                override fun newArray(size: Int) = arrayOfNulls<DisplayInfo>(size)
            }
    }
}
