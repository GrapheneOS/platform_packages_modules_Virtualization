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
import android.util.Log
import com.android.system.virtualmachine.flags.Flags
import com.android.virtualization.terminal.DebianServiceImpl
import com.android.virtualization.terminal.PortsStateManager
import com.android.virtualization.terminal.StorageBalloonWorker
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
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

data class OpenPort(val port: Int, val name: String)

class GuestAgentController(private val context: Context, private val scope: CoroutineScope) {
    private var server: Server? = null
    private var debianService: DebianServiceImpl? = null
    private val portsStateManager = PortsStateManager.getInstance(context)

    private val _ports = MutableStateFlow<List<OpenPort>>(emptyList())
    val ports: StateFlow<List<OpenPort>> = _ports.asStateFlow()

    private val portsListener =
        object : PortsStateManager.Listener {
            override fun onPortsStateUpdated(oldActivePorts: Set<Int>, newActivePorts: Set<Int>) {
                updatePortsState()
            }
        }

    fun start(ipAddress: String?) {
        startDebianServer(ipAddress)
        portsStateManager.registerListener(portsListener)
        updatePortsState()
    }

    fun stop() {
        portsStateManager.unregisterListener(portsListener)
        stopDebianServer()
    }

    fun shutdownVm() {
        debianService?.shutdownDebian()
    }

    fun enablePortForwarding(port: Int, enable: Boolean) {
        portsStateManager.updateEnabledPort(port, enable)
    }

    private fun updatePortsState() {
        val activePorts = portsStateManager.getActivePorts()
        val openPorts =
            activePorts.mapNotNull { port ->
                portsStateManager.getActivePortInfo(port)?.let { OpenPort(it.port, it.comm) }
            }
        _ports.value = openPorts
    }

    private fun startDebianServer(ipAddress: String?) {
        val interceptor: ServerInterceptor =
            object : ServerInterceptor {
                override fun <ReqT, RespT> interceptCall(
                    call: ServerCall<ReqT?, RespT?>,
                    headers: Metadata?,
                    next: ServerCallHandler<ReqT?, RespT?>,
                ): ServerCall.Listener<ReqT?> {
                    val remoteAddr =
                        call.attributes.get(Grpc.TRANSPORT_ATTR_REMOTE_ADDR) as? InetSocketAddress

                    if (ipAddress == null || remoteAddr?.address?.hostAddress == ipAddress) {
                        // Allow the request only if it is from VM (or if ipAddress is null/unknown)
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
            debianService = DebianServiceImpl(context)
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

        scope.launch(Dispatchers.IO) {
            // TODO(b/373533555): we can use mDNS for that.
            val debianServicePortFile = File(context.filesDir, "debian_service_port")
            try {
                FileOutputStream(debianServicePortFile).use { writer ->
                    writer.write(server!!.port.toString().toByteArray())
                }
            } catch (e: IOException) {
                Log.d(TAG, "cannot write grpc port number", e)
            }
        }

        if (Flags.terminalStorageBalloon()) {
            StorageBalloonWorker.start(context, debianService!!)
        }
    }

    private fun stopDebianServer() {
        debianService?.killForwarderHost()
        debianService?.closeStorageBalloonRequestQueue()
        server?.shutdown()
        server = null
        debianService = null
    }

    companion object {
        private const val TAG = "GuestAgentController"
    }
}
