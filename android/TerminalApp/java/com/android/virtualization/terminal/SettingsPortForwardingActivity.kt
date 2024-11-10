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

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.drawable.Icon
import android.os.Bundle
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.ActivityCompat
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView

class SettingsPortForwardingActivity : AppCompatActivity() {
    val TAG: String = "VmTerminalApp"
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.settings_port_forwarding)

        val settingsPortForwardingItems = ArrayList<SettingsPortForwardingItem>()

        val sharedPref = this.getSharedPreferences(
            getString(R.string.preference_file_key), Context.MODE_PRIVATE
        )

        val ports =
            sharedPref.getStringSet(
                getString(R.string.preference_forwarding_ports),
                HashSet<String>()
            )

        for (port in ports!!) {
            val enabled =
                sharedPref.getBoolean(
                    getString(R.string.preference_forwarding_port_is_enabled) + port,
                    false
                )
            settingsPortForwardingItems.add(SettingsPortForwardingItem(port.toInt(), enabled));
        }

        val settingsPortForwardingAdapter =
            SettingsPortForwardingAdapter(settingsPortForwardingItems, this)

        val recyclerView: RecyclerView = findViewById(R.id.settings_port_forwarding_recycler_view)
        recyclerView.layoutManager = LinearLayoutManager(this)
        recyclerView.adapter = settingsPortForwardingAdapter

        // TODO: implement intent for accept, deny and tap to the notification
        // Currently show a mock notification of a port opening
        val terminalIntent = Intent()
        val pendingIntent = PendingIntent.getActivity(
            this, 0, terminalIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
        val notification =
            Notification.Builder(this, TAG)
                .setChannelId(TAG)
                .setSmallIcon(R.drawable.ic_launcher_foreground)
                .setContentTitle(resources.getString(R.string.settings_port_forwarding_notification_title))
                .setContentText(
                    resources.getString(
                        R.string.settings_port_forwarding_notification_content,
                        8080
                    )
                )
                .addAction(
                    Notification.Action.Builder(
                        Icon.createWithResource(resources, R.drawable.ic_launcher_foreground),
                        resources.getString(R.string.settings_port_forwarding_notification_accept),
                        pendingIntent
                    ).build()
                )
                .addAction(
                    Notification.Action.Builder(
                        Icon.createWithResource(resources, R.drawable.ic_launcher_foreground),
                        resources.getString(R.string.settings_port_forwarding_notification_deny),
                        pendingIntent
                    ).build()
                )
                .build()

        with(NotificationManager.from(this)) {
            if (ActivityCompat.checkSelfPermission(
                    this@SettingsPortForwardingActivity, Manifest.permission.POST_NOTIFICATIONS
                ) == PackageManager.PERMISSION_GRANTED
            ) {
                notify(0, notification)
            }
        }
    }
}