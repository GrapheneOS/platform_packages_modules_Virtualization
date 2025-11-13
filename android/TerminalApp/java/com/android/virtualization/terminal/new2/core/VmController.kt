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
import android.os.SystemProperties
import android.system.virtualmachine.VirtualMachine
import android.system.virtualmachine.VirtualMachineCallback
import android.system.virtualmachine.VirtualMachineException
import android.system.virtualmachine.VirtualMachineManager
import android.util.Log
import com.android.virtualization.terminal.CertificateUtils
import com.android.virtualization.terminal.ConfigJson
import com.android.virtualization.terminal.InstalledImage
import com.android.virtualization.terminal.TerminalThreadFactory
import com.android.virtualization.terminal.new2.util.LoggingMutableStateFlow
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

object VmController {

    private lateinit var context: Context
    private val repositoryScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val _vmState =
        LoggingMutableStateFlow<VmState>(MutableStateFlow(VmState.Ready), "VmController")
    val vmState: StateFlow<VmState> = _vmState.asStateFlow()

    private var virtualMachine: VirtualMachine? = null

    fun initialize(context: Context) {
        this.context = context.applicationContext
        val key = CertificateUtils.createOrGetKey()
        CertificateUtils.writeCertificateToFile(this.context, key.certificate)
    }

    fun reset() {
        if (_vmState.value == VmState.Stopped || _vmState.value is VmState.Error) {
            _vmState.value = VmState.Ready
        }
    }

    fun start() {
        if (_vmState.value is VmState.Running || _vmState.value is VmState.Starting) return

        val intent = Intent(context, VmService::class.java)
        context.startForegroundService(intent)
        repositoryScope.launch {
            _vmState.value = VmState.Starting
            try {
                val image = InstalledImage.getDefault(context)
                val json = ConfigJson.from(context, image.configPath)
                val config = json.toConfigBuilder(context).build()

                val vmm = context.getSystemService(VirtualMachineManager::class.java)!!
                val vmName = config.customImageConfig!!.name!!

                try {
                    vmm.get(vmName)?.let { // Clean up existing VM if it's not stopped
                        if (it.status != VirtualMachine.STATUS_STOPPED) {
                            Log.e("VmController", "stopping vm because it is not stopped")
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

                val callback =
                    object : VirtualMachineCallback {
                        override fun onPayloadStarted(vm: VirtualMachine) {}

                        override fun onPayloadReady(vm: VirtualMachine) {}

                        override fun onPayloadFinished(vm: VirtualMachine, exitCode: Int) {}

                        override fun onError(vm: VirtualMachine, errorCode: Int, message: String) {
                            Log.e("VmController", "VM error: $message ($errorCode)")
                            _vmState.value = VmState.Error(RuntimeException("VM error: $message"))
                        }

                        override fun onStopped(vm: VirtualMachine, reason: Int) {
                            _vmState.value = VmState.Stopped
                        }
                    }

                vm.setCallback(Executors.newSingleThreadExecutor(), callback)
                vm.run()

                val timeout = json.getBootTimeoutSecs() ?: 60
                val effectiveTimeout = if (IS_EMULATOR) (timeout * 10) else timeout

                startTtydDiscovery(effectiveTimeout.toLong())
            } catch (e: Exception) {
                Log.e("VmController", "Failed to start VM", e)
                _vmState.value = VmState.Error(e)
            }
        }
    }

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
