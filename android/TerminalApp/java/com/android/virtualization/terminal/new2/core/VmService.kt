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
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.graphics.drawable.Icon
import android.os.IBinder
import android.os.PowerManager
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.ui.MainActivity
import com.android.virtualization.terminal.new2.ui.main.SettingsViewModel
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

class VmService : LifecycleService() {

    private var wakeLock: PowerManager.WakeLock? = null
    private var wakeLockTimeoutJob: Job? = null
    private var isAppInForeground = true

    private val sharedPrefListener =
        SharedPreferences.OnSharedPreferenceChangeListener { _, key ->
            if (key == SettingsViewModel.KEY_KEEP_AWAKE) {
                updateWakeLockState()
            }
        }

    override fun onBind(intent: Intent): IBinder? {
        super.onBind(intent)
        return null
    }

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
        startForeground(NOTIFICATION_ID, createRunNotification(VmState.Ready))

        val powerManager = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = powerManager.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "TerminalApp:KeepAwake")

        val sharedPref = getSharedPreferences(SettingsViewModel.PREFS_NAME, Context.MODE_PRIVATE)
        sharedPref.registerOnSharedPreferenceChangeListener(sharedPrefListener)

        monitorInstaller()
        monitorVmController()
        monitorPorts()
        updateWakeLockState()
    }

    override fun onDestroy() {
        super.onDestroy()
        val sharedPref = getSharedPreferences(SettingsViewModel.PREFS_NAME, Context.MODE_PRIVATE)
        sharedPref.unregisterOnSharedPreferenceChangeListener(sharedPrefListener)
        releaseWakeLock()
    }

    private fun updateWakeLockState() {
        val sharedPref = getSharedPreferences(SettingsViewModel.PREFS_NAME, Context.MODE_PRIVATE)
        val keepAwakeMinutes = sharedPref.getInt(SettingsViewModel.KEY_KEEP_AWAKE, 0)

        if (isAppInForeground || keepAwakeMinutes == 0) {
            releaseWakeLock()
        } else {
            acquireWakeLock(keepAwakeMinutes)
        }

        // Update the main notification to show the "Keep awake" status
        val vmState = VmController.vmState.value
        if (vmState.isAlive) {
            val notification = createRunNotification(vmState)
            getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification)
        }
    }

    private fun acquireWakeLock(minutes: Int) {
        if (wakeLock?.isHeld == false) {
            wakeLock?.acquire()
        }
        wakeLockTimeoutJob?.cancel()
        wakeLockTimeoutJob =
            lifecycleScope.launch {
                delay(minutes.toLong() * 60 * 1000)
                // When timeout occurs, reset the setting to Off and release WakeLock
                val sharedPref =
                    getSharedPreferences(SettingsViewModel.PREFS_NAME, Context.MODE_PRIVATE)
                sharedPref.edit().putInt(SettingsViewModel.KEY_KEEP_AWAKE, 0).apply()
                // The sharedPrefListener will trigger releaseWakeLock via updateWakeLockState()
            }
    }

    private fun releaseWakeLock() {
        if (wakeLock?.isHeld == true) {
            wakeLock?.release()
        }
        wakeLockTimeoutJob?.cancel()
        wakeLockTimeoutJob = null
    }

    private fun monitorPorts() {
        lifecycleScope.launch {
            var previousPorts = emptySet<Int>()
            VmController.ports.collect { currentPorts ->
                val currentPortSet = currentPorts.map { it.port }.toSet()
                val newPorts = currentPorts.filter { it.port !in previousPorts }

                newPorts.forEach { port ->
                    if (!port.isForwarded) {
                        val notification = createPortNotification(port)
                        getSystemService(NotificationManager::class.java)
                            .notify(port.port, notification)
                    }
                }

                val closedPorts = previousPorts - currentPortSet
                closedPorts.forEach { portId ->
                    getSystemService(NotificationManager::class.java).cancel(portId)
                }

                previousPorts = currentPortSet
            }
        }
    }

    private fun createPortNotification(port: OpenPort): Notification {
        val intent =
            Intent(this, MainActivity::class.java).apply {
                action = MainActivity.ACTION_OPEN_SETTINGS_PORT
                flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
            }
        val pendingIntent =
            PendingIntent.getActivity(
                this,
                port.port,
                intent,
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )

        val acceptIntent =
            Intent(this, VmService::class.java).apply {
                action = ACTION_PORT_FORWARD
                putExtra(EXTRA_PORT, port.port)
                putExtra(EXTRA_ENABLE, true)
            }
        val acceptPending =
            PendingIntent.getService(
                this,
                port.port * 2,
                acceptIntent,
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )

        val denyIntent =
            Intent(this, VmService::class.java).apply {
                action = ACTION_PORT_FORWARD
                putExtra(EXTRA_PORT, port.port)
                putExtra(EXTRA_ENABLE, false)
            }
        val denyPending =
            PendingIntent.getService(
                this,
                port.port * 2 + 1,
                denyIntent,
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )

        return Notification.Builder(this, PORT_CHANNEL_ID)
            .setContentTitle("New Port Opened")
            .setContentText("Port ${port.port} (${port.name}) is now open. Allow forwarding?")
            .setSmallIcon(R.drawable.ic_terminal)
            .setContentIntent(pendingIntent)
            .addAction(Notification.Action.Builder(null, "Accept", acceptPending).build())
            .addAction(Notification.Action.Builder(null, "Deny", denyPending).build())
            .build()
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
            ACTION_APP_FOREGROUND -> {
                isAppInForeground = true
                updateWakeLockState()
            }
            ACTION_APP_BACKGROUND -> {
                isAppInForeground = false
                updateWakeLockState()
            }
            ACTION_CANCEL_INSTALL -> Installer.cancelInstall()
            ACTION_STOP_VM -> VmController.stop()
            ACTION_PORT_FORWARD -> {
                val port = intent.getIntExtra(EXTRA_PORT, -1)
                val enable = intent.getBooleanExtra(EXTRA_ENABLE, false)
                if (port != -1) {
                    VmController.enablePortForwarding(port, enable)
                    getSystemService(NotificationManager::class.java).cancel(port)
                }
            }
        }
        super.onStartCommand(intent, flags, startId)
        return START_NOT_STICKY
    }

    private fun createNotificationChannel() {
        val manager = getSystemService(NotificationManager::class.java)
        val channel =
            NotificationChannel(CHANNEL_ID, "VM Service", NotificationManager.IMPORTANCE_LOW)
        manager.createNotificationChannel(channel)
        val portChannel =
            NotificationChannel(
                PORT_CHANNEL_ID,
                "Port Forwarding",
                NotificationManager.IMPORTANCE_HIGH,
            )
        manager.createNotificationChannel(portChannel)
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
                        Icon.createWithResource(this, R.drawable.ic_close),
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
                            Icon.createWithResource(this, R.drawable.ic_close),
                            getString(R.string.notif_action_close),
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
        const val ACTION_APP_FOREGROUND = "ACTION_APP_FOREGROUND"
        const val ACTION_APP_BACKGROUND = "ACTION_APP_BACKGROUND"
        private const val ACTION_CANCEL_INSTALL = "ACTION_CANCEL_INSTALL"
        private const val ACTION_STOP_VM = "ACTION_STOP_VM"
        private const val ACTION_PORT_FORWARD = "ACTION_PORT_FORWARD"
        private const val EXTRA_PORT = "EXTRA_PORT"
        private const val EXTRA_ENABLE = "EXTRA_ENABLE"
        private const val CHANNEL_ID = "vm_service_channel"
        private const val PORT_CHANNEL_ID = "vm_port_channel"
    }
}
