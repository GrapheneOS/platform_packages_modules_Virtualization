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
import android.os.SystemProperties
import android.system.virtualmachine.VirtualMachine
import android.system.virtualmachine.VirtualMachineCustomImageConfig
import android.system.virtualmachine.VirtualMachineCustomImageConfig.AudioConfig
import android.system.virtualmachine.VirtualMachineException
import android.util.Log
import android.widget.Toast
import androidx.annotation.WorkerThread
import com.android.system.virtualmachine.flags.Flags
import com.android.virtualization.terminal.MainActivity.Companion.PREFIX
import com.android.virtualization.terminal.MainActivity.Companion.TAG
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
import java.net.InetSocketAddress
import java.net.SocketAddress
import java.nio.file.Files
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class VmLauncherService : Service() {
    private lateinit var executorService: ExecutorService
    private lateinit var image: InstalledImage

    // TODO: using lateinit for some fields to avoid null
    private var virtualMachine: VirtualMachine? = null
    private var server: Server? = null
    private var debianService: DebianServiceImpl? = null
    private var portNotifier: PortNotifier? = null
    private var runner: Runner? = null

    interface VmLauncherServiceCallback {
        fun onVmStart()

        fun onTerminalAvailable(info: TerminalInfo)

        fun onVmStop()

        fun onVmError()
    }

    override fun onBind(intent: Intent?): IBinder? {
        return null
    }

    override fun onCreate() {
        super.onCreate()
        executorService = Executors.newCachedThreadPool(TerminalThreadFactory(applicationContext))
        image = InstalledImage.getDefault(this)
    }

    override fun onStartCommand(intent: Intent, flags: Int, startId: Int): Int {
        val resultReceiver =
            intent.getParcelableExtra<ResultReceiver>(
                Intent.EXTRA_RESULT_RECEIVER,
                ResultReceiver::class.java,
            )!!

        when (intent.action) {
            ACTION_START_VM -> {
                val notification =
                    intent.getParcelableExtra<Notification>(
                        EXTRA_NOTIFICATION,
                        Notification::class.java,
                    )!!

                val displayInfo =
                    intent.getParcelableExtra(EXTRA_DISPLAY_INFO, DisplayInfo::class.java)!!

                // Note: this doesn't always do the resizing. If the current image size is the same
                // as the requested size which is rounded up to the page alignment, resizing is not
                // done.
                val diskSize = intent.getLongExtra(EXTRA_DISK_SIZE, image.getSize())

                executorService.submit({
                    doStart(notification, displayInfo, diskSize, resultReceiver)
                })
            }
            ACTION_SHUTDOWN_VM -> executorService.submit({ doShutdown(resultReceiver) })
            else -> {
                Log.e(TAG, "Unknown command " + intent.action)
                stopSelf()
            }
        }

        return START_NOT_STICKY
    }

    @WorkerThread
    private fun doStart(
        notification: Notification,
        displayInfo: DisplayInfo,
        diskSize: Long,
        resultReceiver: ResultReceiver,
    ) {
        if (virtualMachine != null) {
            Log.d(TAG, "VM instance is already started")
            return
        }

        val image = InstalledImage.getDefault(this)
        val json = ConfigJson.from(this, image.configPath)
        val configBuilder = json.toConfigBuilder(this)
        val customImageConfigBuilder = json.toCustomImageConfigBuilder(this)
        image.resize(diskSize)

        customImageConfigBuilder.setAudioConfig(
            AudioConfig.Builder().setUseSpeaker(true).setUseMicrophone(true).build()
        )
        if (overrideConfigIfNecessary(customImageConfigBuilder, displayInfo)) {
            configBuilder.setCustomImageConfig(customImageConfigBuilder.build())
        }
        val config = configBuilder.build()

        runner =
            try {
                Runner.create(this, config)
            } catch (e: VirtualMachineException) {
                throw RuntimeException("cannot create runner", e)
            }

        virtualMachine = runner!!.vm
        val mbc = MemBalloonController(this, virtualMachine!!)
        mbc.start()

        runner!!.exitStatus.thenAcceptAsync { success: Boolean ->
            mbc.stop()
            resultReceiver.send(if (success) RESULT_STOP else RESULT_ERROR, null)
            stopSelf()
        }
        val logDir = getFileStreamPath(virtualMachine!!.name + ".log").toPath()
        Logger.setup(virtualMachine!!, logDir, executorService)

        startForeground(this.hashCode(), notification)

        resultReceiver.send(RESULT_START, null)

        portNotifier = PortNotifier(this)

        getTerminalServiceInfo()
            .thenAcceptAsync(
                { info ->
                    val ipAddress = info.hostAddresses[0].hostAddress
                    val port = info.port
                    val bundle = Bundle()
                    bundle.putString(KEY_TERMINAL_IPADDRESS, ipAddress)
                    bundle.putInt(KEY_TERMINAL_PORT, port)
                    resultReceiver.send(RESULT_TERMINAL_AVAIL, bundle)
                    startDebianServer(ipAddress)
                },
                executorService,
            )
            .exceptionallyAsync(
                { e ->
                    Log.e(TAG, "Failed to start VM", e)
                    resultReceiver.send(RESULT_ERROR, null)
                    stopSelf()
                    null
                },
                executorService,
            )
    }

    private fun getTerminalServiceInfo(): CompletableFuture<NsdServiceInfo> {
        val executor = Executors.newSingleThreadExecutor(TerminalThreadFactory(applicationContext))
        val nsdManager = getSystemService<NsdManager?>(NsdManager::class.java)
        val queryInfo = NsdServiceInfo()
        queryInfo.serviceType = "_http._tcp"
        queryInfo.serviceName = "ttyd"
        var resolvedInfo = CompletableFuture<NsdServiceInfo>()

        nsdManager.registerServiceInfoCallback(
            queryInfo,
            executor,
            object : NsdManager.ServiceInfoCallback {
                var found: Boolean = false

                override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {}

                override fun onServiceInfoCallbackUnregistered() {
                    executor.shutdown()
                }

                override fun onServiceLost() {}

                override fun onServiceUpdated(info: NsdServiceInfo) {
                    Log.i(TAG, "Service found: $info")
                    if (!found) {
                        found = true
                        nsdManager.unregisterServiceInfoCallback(this)
                        resolvedInfo.complete(info)
                    }
                }
            },
        )

        resolvedInfo.orTimeout(VM_BOOT_TIMEOUT_SECONDS.toLong(), TimeUnit.SECONDS)
        return resolvedInfo
    }

    private fun createNotificationForTerminalClose(): Notification {
        val stopIntent = Intent()
        stopIntent.setClass(this, VmLauncherService::class.java)
        stopIntent.setAction(ACTION_SHUTDOWN_VM)
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
        } else if (Files.exists(ImageArchive.getSdcardPathForTesting().resolve("gfxstream"))) {
            // TODO: check if the configuration is right. current config comes from cuttlefish's one
            builder.setGpuConfig(
                VirtualMachineCustomImageConfig.GpuConfig.Builder()
                    .setBackend("gfxstream")
                    .setRendererUseEgl(false)
                    .setRendererUseGles(false)
                    .setRendererUseGlx(false)
                    .setRendererUseSurfaceless(true)
                    .setRendererUseVulkan(true)
                    .setContextTypes(arrayOf<String>("gfxstream-vulkan", "gfxstream-composer"))
                    .build()
            )
            Toast.makeText(this, "gfxstream", Toast.LENGTH_SHORT).show()
            changed = true
        }

        // Set the initial display size
        // TODO(jeongik): set up the display size on demand
        if (Flags.terminalGuiSupport() && displayInfo != null) {
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

        executorService.execute(
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

        if (Flags.terminalStorageBalloon()) {
            StorageBalloonWorker.start(this, debianService!!)
        }
    }

    @WorkerThread
    private fun doShutdown(resultReceiver: ResultReceiver) {
        if (debianService != null && debianService!!.shutdownDebian()) {
            // During shutdown, change the notification content to indicate that it's closing
            val notification = createNotificationForTerminalClose()
            getSystemService<NotificationManager?>(NotificationManager::class.java)
                .notify(this.hashCode(), notification)
            runner?.exitStatus?.thenAcceptAsync { success: Boolean ->
                resultReceiver.send(if (success) RESULT_STOP else RESULT_ERROR, null)
                stopSelf()
            }
        } else {
            // If there is no Debian service or it fails to shutdown, just stop the service.
            stopSelf()
        }
    }

    private fun stopDebianServer() {
        debianService?.killForwarderHost()
        debianService?.closeStorageBalloonRequestQueue()
        server?.shutdown()
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
            virtualMachine = null
        }
        executorService.shutdownNow()
        super.onDestroy()
    }

    companion object {
        private const val ACTION_START_VM: String = PREFIX + "ACTION_START_VM"
        private const val EXTRA_NOTIFICATION = PREFIX + "EXTRA_NOTIFICATION"
        private const val EXTRA_DISPLAY_INFO = PREFIX + "EXTRA_DISPLAY_INFO"
        private const val EXTRA_DISK_SIZE = PREFIX + "EXTRA_DISK_SIZE"

        private const val ACTION_SHUTDOWN_VM: String = PREFIX + "ACTION_SHUTDOWN_VM"

        private const val RESULT_START = 0
        private const val RESULT_STOP = 1
        private const val RESULT_ERROR = 2
        private const val RESULT_TERMINAL_AVAIL = 3

        private const val KEY_TERMINAL_IPADDRESS = "address"
        private const val KEY_TERMINAL_PORT = "port"

        private val VM_BOOT_TIMEOUT_SECONDS: Int =
            {
                val deviceName = SystemProperties.get("ro.product.vendor.device", "")
                val cuttlefish = deviceName.startsWith("vsoc_")
                val goldfish = deviceName.startsWith("emu64")

                if (cuttlefish || goldfish) {
                    3 * 60
                } else {
                    30
                }
            }()

        private fun prepareIntent(context: Context, callback: VmLauncherServiceCallback): Intent {
            val intent = Intent(context.getApplicationContext(), VmLauncherService::class.java)
            val resultReceiver =
                object : ResultReceiver(Handler(Looper.myLooper()!!)) {
                    override fun onReceiveResult(resultCode: Int, resultData: Bundle?) {
                        when (resultCode) {
                            RESULT_START -> callback.onVmStart()
                            RESULT_TERMINAL_AVAIL -> {
                                val ipAddress = resultData!!.getString(KEY_TERMINAL_IPADDRESS)
                                val port = resultData!!.getInt(KEY_TERMINAL_PORT)
                                callback.onTerminalAvailable(TerminalInfo(ipAddress!!, port))
                            }
                            RESULT_STOP -> callback.onVmStop()
                            RESULT_ERROR -> callback.onVmError()
                            else -> Log.e(TAG, "unknown result code: " + resultCode)
                        }
                    }
                }

            val parcel = Parcel.obtain()
            resultReceiver.writeToParcel(parcel, 0)
            parcel.setDataPosition(0)
            intent.putExtra(
                Intent.EXTRA_RESULT_RECEIVER,
                ResultReceiver.CREATOR.createFromParcel(parcel).also { parcel.recycle() },
            )
            return intent
        }

        fun getIntentForStart(
            context: Context,
            callback: VmLauncherServiceCallback,
            notification: Notification?,
            displayInfo: DisplayInfo,
            diskSize: Long?,
        ): Intent {
            val i = prepareIntent(context, callback)
            i.setAction(ACTION_START_VM)
            i.putExtra(EXTRA_NOTIFICATION, notification)
            i.putExtra(EXTRA_DISPLAY_INFO, displayInfo)
            if (diskSize != null) {
                i.putExtra(EXTRA_DISK_SIZE, diskSize)
            }
            return i
        }

        fun getIntentForShutdown(context: Context, callback: VmLauncherServiceCallback): Intent {
            val i = prepareIntent(context, callback)
            i.setAction(ACTION_SHUTDOWN_VM)
            return i
        }
    }
}

data class TerminalInfo(val ipAddress: String, val port: Int)

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
