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
import android.app.PendingIntent
import android.content.Intent
import android.os.IBinder
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.ui.MainActivity
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

class VmService : LifecycleService() {

    override fun onBind(intent: Intent): IBinder? {
        super.onBind(intent)
        return null
    }

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
        startForeground(NOTIFICATION_ID, createRunNotification(VmState.Ready))

        monitorInstaller()
        monitorVmController()
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private fun monitorInstaller() {
        lifecycleScope.launch {
            Installer.installState
                .flatMapLatest { state ->
                    if (state is InstallState.Installing) {
                        state.progress.map { p -> state to p }
                    } else {
                        flowOf(state to -1L)
                    }
                }
                .collect { (state, progress) ->
                    if (state is InstallState.Installing) {
                        val notification = createInstallNotification(state, progress)
                        getSystemService(NotificationManager::class.java)
                            .notify(NOTIFICATION_ID, notification)
                    }
                    checkStopSelf()
                }
        }
    }

    private fun monitorVmController() {
        lifecycleScope.launch {
            VmController.vmState.collect { state ->
                if (state.isAlive) {
                    val notification = createRunNotification(state)
                    getSystemService(NotificationManager::class.java)
                        .notify(NOTIFICATION_ID, notification)
                }
                checkStopSelf()
            }
        }
    }

    private fun checkStopSelf() {
        val installState = Installer.installState.value
        val vmState = VmController.vmState.value

        val isInstalling = installState is InstallState.Installing

        if (!isInstalling && !vmState.isAlive) {
            stopSelf()
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_CANCEL_INSTALL -> Installer.cancelInstall()
            ACTION_STOP_VM -> VmController.stop()
        }
        super.onStartCommand(intent, flags, startId)
        return START_NOT_STICKY
    }

    private fun createNotificationChannel() {
        val channel =
            NotificationChannel(CHANNEL_ID, "VM Service", NotificationManager.IMPORTANCE_LOW)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private fun getMainActivityPendingIntent(): PendingIntent {
        val intent = Intent(this, MainActivity::class.java)
        return PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE)
    }

    private fun createInstallNotification(
        state: InstallState.Installing,
        progress: Long,
    ): Notification {
        val total = state.totalBytes.toInt()
        val current = progress.toInt()
        val cancelIntent =
            Intent(this, VmService::class.java).apply { action = ACTION_CANCEL_INSTALL }
        val cancelPending =
            PendingIntent.getService(this, 1, cancelIntent, PendingIntent.FLAG_IMMUTABLE)

        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("Terminal Service")
            .setContentText("Installing...")
            .setSmallIcon(R.drawable.ic_terminal)
            .setContentIntent(getMainActivityPendingIntent())
            .setProgress(total, current, total <= 0)
            .addAction(
                Notification.Action.Builder(
                        R.drawable.ic_close,
                        getString(android.R.string.cancel),
                        cancelPending,
                    )
                    .build()
            )
            .build()
    }

    private fun createRunNotification(state: VmState): Notification {
        val stopIntent = Intent(this, VmService::class.java).apply { action = ACTION_STOP_VM }
        val stopPending =
            PendingIntent.getService(this, 2, stopIntent, PendingIntent.FLAG_IMMUTABLE)

        val builder =
            Notification.Builder(this, CHANNEL_ID)
                .setContentTitle("Terminal Service")
                .setSmallIcon(R.drawable.ic_terminal)
                .setContentIntent(getMainActivityPendingIntent())
                .addAction(
                    Notification.Action.Builder(
                            R.drawable.ic_close,
                            getString(R.string.service_notification_quit_action),
                            stopPending,
                        )
                        .build()
                )

        if (state is VmState.Starting) {
            builder.setContentText("Terminal is starting...")
        } else if (state is VmState.Running) {
            builder.setContentText("Terminal is running")
        } else if (state is VmState.Stopping) {
            builder.setContentText("Terminal is shutting down")
        }

        return builder.build()
    }

    companion object {
        private const val NOTIFICATION_ID = 1
        private const val ACTION_CANCEL_INSTALL = "ACTION_CANCEL_INSTALL"
        private const val ACTION_STOP_VM = "ACTION_STOP_VM"
        private const val CHANNEL_ID = "vm_service_channel"
    }
}
