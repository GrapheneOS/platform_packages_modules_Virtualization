/*
 * Copyright (C) 2025 The Android Open Source Project
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
package com.android.virtualization.terminal.new2.core

import android.content.Context
import android.content.Intent
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.SystemProperties
import android.system.virtualizationcommon.IGuestAgent
import android.system.virtualmachine.VirtualMachine
import android.system.virtualmachine.VirtualMachineCallback
import android.system.virtualmachine.VirtualMachineCustomImageConfig
import android.system.virtualmachine.VirtualMachineException
import android.system.virtualmachine.VirtualMachineManager
import android.util.Log
import com.android.system.virtualmachine.flags.Flags
import com.android.virtualization.debian.aidl.IDebianService
import com.android.virtualization.terminal.AndroidToVmBridge
import com.android.virtualization.terminal.CertificateUtils
import com.android.virtualization.terminal.ConfigJson
import com.android.virtualization.terminal.DisplayInfo
import com.android.virtualization.terminal.GraphicsManager
import com.android.virtualization.terminal.ImageArchive
import com.android.virtualization.terminal.InstalledImage
import com.android.virtualization.terminal.Logger
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.TerminalThreadFactory
import com.android.virtualization.terminal.new2.util.LoggingMutableStateFlow
import java.nio.file.Files
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

object VmController {
    private val TAG = "VmController"

    private lateinit var context: Context
    private val repositoryScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val _vmState = LoggingMutableStateFlow<VmState>(MutableStateFlow(VmState.Ready), TAG)
    val vmState: StateFlow<VmState> = _vmState.asStateFlow()

    private val _sessionDiscarded = MutableSharedFlow<String>()
    val sessionDiscarded: SharedFlow<String> = _sessionDiscarded.asSharedFlow()

    private var guestAgentController: GuestAgentController? = null
    val ports: StateFlow<List<OpenPort>>
        get() = guestAgentController?.ports ?: MutableStateFlow(emptyList())

    var virtualMachine: VirtualMachine? = null
        private set

    fun initialize(context: Context) {
        this.context = context.applicationContext
        val key = CertificateUtils.createOrGetKey()
        CertificateUtils.writeCertificateToFile(this.context, key.certificate)
        guestAgentController = GuestAgentController(this.context, repositoryScope)
    }

    fun reset() {
        if (_vmState.value == VmState.Stopped || _vmState.value is VmState.Error) {
            _vmState.value = VmState.Ready
        }
    }

    fun enablePortForwarding(port: Int, enable: Boolean) {
        guestAgentController?.enablePortForwarding(port, enable)
    }

    val graphicsAccelerationType: GraphicsManager.AccelerationType
        get() = GraphicsManager.getInstance(context).accelerationType

    val isGraphicsAccelerationSupported: Boolean
        get() = GraphicsManager.getInstance(context).isGfxstreamSupported

    fun setGraphicsAccelerationType(type: GraphicsManager.AccelerationType) {
        GraphicsManager.getInstance(context).accelerationType = type
    }

    fun shutdownVm() {
        guestAgentController?.shutdownVm()
    }

    fun requestSessionDiscard(sessionId: String) {
        repositoryScope.launch { _sessionDiscarded.emit(sessionId) }
    }

    fun start(displayInfo: DisplayInfo) {
        if (_vmState.value is VmState.Running || _vmState.value is VmState.Starting) return

        val intent = Intent(context, VmService::class.java)
        context.startForegroundService(intent)
        repositoryScope.launch {
            _vmState.value = VmState.Starting
            try {
                val image = InstalledImage.getDefault(context)
                val json = ConfigJson.from(context, image.configPath)
                val configBuilder = json.toConfigBuilder(context)
                val customImageConfigBuilder = json.toCustomImageConfigBuilder(context)

                // Override config for Display
                setDisplayConfig(customImageConfigBuilder, displayInfo)
                setGpuConfig(context, customImageConfigBuilder)
                configBuilder.setCustomImageConfig(customImageConfigBuilder.build())

                val config = configBuilder.build()

                val vmm = context.getSystemService(VirtualMachineManager::class.java)!!
                val vmName = config.customImageConfig!!.name!!

                try {
                    vmm.get(vmName)?.let { // Clean up existing VM if it's not stopped
                        if (it.status != VirtualMachine.STATUS_STOPPED) {
                            Log.e(TAG, "stopping vm because it is not stopped")
                            it.stop()
                        }
                        // TODO: revisit this to see if we can omit this step.
                        vmm.delete(vmName)
                    }
                } catch (e: VirtualMachineException) {
                    // Ignore if VM doesn't exist
                }

                val vm = vmm.create(vmName, config)
                virtualMachine = vm
                Logger.setup(context, vm, Executors.newSingleThreadExecutor())

                val callback =
                    object : VirtualMachineCallback {
                        override fun onPayloadStarted(vm: VirtualMachine) {}

                        override fun onPayloadReady(vm: VirtualMachine) {}

                        override fun onPayloadFinished(vm: VirtualMachine, exitCode: Int) {}

                        override fun onError(vm: VirtualMachine, errorCode: Int, message: String) {
                            Log.e(TAG, "VM error: $message ($errorCode)")
                            _vmState.value = VmState.Error(RuntimeException("VM error: $message"))
                            guestAgentController?.stop()
                        }

                        override fun onStopped(vm: VirtualMachine, reason: Int) {
                            // TODO: b/438355564 STOP_REASON_KILLED should also be an error
                            if (
                                reason == VirtualMachineCallback.STOP_REASON_SHUTDOWN ||
                                    reason == VirtualMachineCallback.STOP_REASON_KILLED
                            ) {
                                _vmState.value = VmState.Stopped
                            } else {
                                _vmState.value =
                                    VmState.Error(
                                        RuntimeException("VM stopped unexpectedly: $reason")
                                    )
                            }
                            guestAgentController?.stop()
                        }

                        override fun onGuestAgentRegistered(
                            vm: VirtualMachine,
                            guestAgent: IGuestAgent,
                        ) {
                            Log.d(TAG, "Guest agent registered. Imply AIDL connection")

                            var binder: IBinder? = null
                            try {
                                binder = vm.connectToVsockServer(IDebianService.VSOCK_PORT)
                            } catch (e: Exception) {
                                _vmState.value =
                                    VmState.Error(
                                        RuntimeException("Failed to connect to guest agent", e)
                                    )
                                return
                            }
                            val debian_service = IDebianService.Stub.asInterface(binder)

                            val cid = vm!!.cid
                            guestAgentController?.start(cid, guestAgent, debian_service)

                            Log.d(TAG, "Guest agent ready")
                        }
                    }

                vm.setCallback(Executors.newSingleThreadExecutor(), callback)
                vm.run()

                if (canUseTtydOverVsock()) {
                    Log.i(TAG, "Connect to ttyd using vsock")
                    val bridge = AndroidToVmBridge(virtualMachine!!.cid)
                    val port = bridge.start()
                    if (port == null) {
                        Log.e(TAG, "Failed to start bridge")
                        _vmState.value = VmState.Error(RuntimeException("Failed to start bridge"))
                    } else {
                        Log.d(TAG, "localhost is running with port=" + port)
                        _vmState.value = VmState.Running("localhost", port, bridge.secretKey)
                    }
                }

                val timeout = json.getBootTimeoutSecs() ?: 60
                val effectiveTimeout = if (IS_EMULATOR) (timeout * 10) else timeout

                if (context.resources.getBoolean(R.bool.soong_generated_cidata)) {
                    repositoryScope.launch {
                        delay(TimeUnit.SECONDS.toMillis(effectiveTimeout.toLong()))
                        if (_vmState.value == VmState.Starting) {
                            _vmState.value =
                                VmState.Error(
                                    RuntimeException("Timed out waiting for terminal service")
                                )
                        }
                    }
                } else {
                    startTtydDiscovery(effectiveTimeout.toLong())
                }
            } catch (e: Exception) {
                Log.e(TAG, "Failed to start VM", e)
                _vmState.value = VmState.Error(e)
                guestAgentController?.stop()
            }
        }
    }

    private fun isGfxstreamEnabled(context: Context): Boolean {
        if (
            Build.isDebuggable() &&
                Files.exists(ImageArchive.getSdcardPathForTesting().resolve("gfxstream"))
        ) {
            return true
        }
        return GraphicsManager.getInstance(context).accelerationType ==
            GraphicsManager.AccelerationType.Gfxstream
    }

    private fun setGpuConfig(context: Context, builder: VirtualMachineCustomImageConfig.Builder) {
        if (isGfxstreamEnabled(context)) {
            builder.addParam("gfxstream_enabled")
            builder.setGpuConfig(
                VirtualMachineCustomImageConfig.GpuConfig.Builder()
                    .setBackend("gfxstream")
                    .setRendererUseEgl(false)
                    .setRendererUseGles(false)
                    .setRendererUseGlx(false)
                    .setRendererUseSurfaceless(true)
                    .setRendererUseVulkan(true)
                    .setContextTypes(arrayOf<String>("gfxstream-vulkan", "gfxstream-composer"))
                    .setRendererFeatures("VulkanDisableCoherentMemoryAndEmulate:enabled")
                    .build()
            )
        }
    }

    private fun setDisplayConfig(
        builder: VirtualMachineCustomImageConfig.Builder,
        displayInfo: DisplayInfo,
    ) {
        if (Flags.terminalGuiSupport()) {
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
        }
    }

    // We still need this logic to retrieve the IP address of the VM to use it for guest agent
    // connection
    // It will be replaced with RpcBinder with vsock.
    private fun startTtydDiscovery(timeoutSecs: Long) {
        val executor =
            Executors.newSingleThreadExecutor(TerminalThreadFactory(context.applicationContext))
        val nsdManager = context.getSystemService<NsdManager>(NsdManager::class.java)!!
        val queryInfo = NsdServiceInfo()
        queryInfo.serviceType = "_http._tcp"
        queryInfo.serviceName = "ttyd"

        var isDiscovered = false

        val callback =
            object : NsdManager.ServiceInfoCallback {
                override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {}

                override fun onServiceInfoCallbackUnregistered() {
                    executor.shutdown()
                }

                override fun onServiceLost() {}

                override fun onServiceUpdated(info: NsdServiceInfo) {
                    val hasUsableAddress = info.hostAddresses.any { !it.isLinkLocalAddress }

                    if (!hasUsableAddress) {
                        return
                    }

                    if (!isDiscovered) {
                        isDiscovered = true
                        try {
                            nsdManager.unregisterServiceInfoCallback(this)
                        } catch (e: IllegalArgumentException) {
                            // Ignore if already unregistered
                        }
                        val ipAddress =
                            info.hostAddresses
                                .firstOrNull { !it.isLinkLocalAddress }!!
                                .hostAddress!!
                        val port = info.port
                        guestAgentController?.start(ipAddress)
                        _vmState.value = VmState.Running(ipAddress, port)
                    }
                }
            }

        nsdManager.registerServiceInfoCallback(queryInfo, executor, callback)

        repositoryScope.launch {
            delay(TimeUnit.SECONDS.toMillis(timeoutSecs))
            if (!isDiscovered) {
                try {
                    nsdManager.unregisterServiceInfoCallback(callback)
                } catch (e: IllegalArgumentException) {
                    // Ignore if already unregistered
                }
                _vmState.value =
                    VmState.Error(RuntimeException("Timed out waiting for terminal service"))
            }
        }
    }

    private fun canUseTtydOverVsock(): Boolean {
        val buildId = InstalledImage.getDefault(context).buildInfo?.buildId ?: 0
        val FIRST_VERSION_SUPPORTS_TTYD_VSOCK = 5106
        return Flags.terminalVmCommunicationRefactoring() &&
            buildId >= FIRST_VERSION_SUPPORTS_TTYD_VSOCK
    }

    fun stop() {
        if (_vmState.value == VmState.Stopped || _vmState.value == VmState.Stopping) return

        repositoryScope.launch {
            _vmState.value = VmState.Stopping
            virtualMachine?.stop()
            _vmState.value = VmState.Stopped
        }
    }

    private val IS_EMULATOR: Boolean =
        {
            val deviceName = SystemProperties.get("ro.product.vendor.device", "")
            val cuttlefish = deviceName.startsWith("vsoc_")
            val goldfish = deviceName.startsWith("emu64")

            cuttlefish || goldfish
        }()
}
