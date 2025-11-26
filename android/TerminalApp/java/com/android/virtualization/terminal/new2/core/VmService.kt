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

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Intent
import android.os.IBinder
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import com.android.virtualization.terminal.R
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

class VmService : LifecycleService() {

    override fun onBind(intent: Intent): IBinder? {
        super.onBind(intent)
        return null
    }

    override fun onCreate() {
        super.onCreate()
        startForeground(NOTIFICATION_ID, createNotification("Terminal Service"))

        lifecycleScope.launch {
            combine(VmController.vmState, Installer.installState) { vmState, installState ->
                    Pair(vmState, installState)
                }
                .collect { (vmState, installState) ->
                    val content =
                        when {
                            installState is InstallState.Installing -> "Installing..."
                            vmState is VmState.Starting -> "Terminal is starting..."
                            vmState is VmState.Running -> "Terminal is running"
                            else -> null
                        }

                    if (content != null) {
                        val notification = createNotification(content)
                        getSystemService(NotificationManager::class.java)
                            .notify(NOTIFICATION_ID, notification)
                    }

                    val isInstalling = installState is InstallState.Installing
                    val isRunning = vmState is VmState.Starting || vmState is VmState.Running

                    if (!isInstalling && !isRunning) {
                        stopSelf()
                    }
                }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        super.onStartCommand(intent, flags, startId)
        return START_NOT_STICKY
    }

    private fun createNotification(content: String): Notification {
        val channelId = "vm_service_channel"
        val channel =
            NotificationChannel(channelId, "VM Service", NotificationManager.IMPORTANCE_LOW)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)

        return Notification.Builder(this, channelId)
            .setContentTitle("Terminal Service")
            .setContentText(content)
            .setSmallIcon(R.drawable.ic_terminal)
            .build()
    }

    companion object {
        private const val NOTIFICATION_ID = 1
    }
}
