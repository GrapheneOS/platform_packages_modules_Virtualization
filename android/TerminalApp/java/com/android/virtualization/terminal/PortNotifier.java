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
import android.app.Notification.Action;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.graphics.drawable.Icon;

import java.util.HashSet;
import java.util.Locale;
import java.util.Set;

/**
 * PortNotifier is responsible for posting a notification when a new open port is detected. User can
 * enable or disable forwarding of the port in notification panel.
 */
class PortNotifier {
    private static final String ACTION_PORT_FORWARDING = "android.virtualization.PORT_FORWARDING";
    private static final String KEY_PORT = "port";
    private static final String KEY_ENABLED = "enabled";

    private final Context mContext;
    private final NotificationManager mNotificationManager;
    private final BroadcastReceiver mReceiver;
    private final PortsStateManager mPortsStateManager;
    private final PortsStateManager.Listener mPortsStateListener;

    public PortNotifier(Context context) {
        mContext = context;
        mNotificationManager = mContext.getSystemService(NotificationManager.class);
        mReceiver = new PortForwardingRequestReceiver();

        mPortsStateManager = PortsStateManager.getInstance(mContext);
        mPortsStateListener =
                new PortsStateManager.Listener() {
                    @Override
                    public void onPortsStateUpdated(
                            Set<Integer> oldActivePorts, Set<Integer> newActivePorts) {
                        Set<Integer> union = new HashSet<>(oldActivePorts);
                        union.addAll(newActivePorts);
                        for (int port : union) {
                            if (!oldActivePorts.contains(port)) {
                                showNotificationFor(port);
                            } else if (!newActivePorts.contains(port)) {
                                discardNotificationFor(port);
                            }
                        }
                    }
                };
        mPortsStateManager.registerListener(mPortsStateListener);

        IntentFilter intentFilter = new IntentFilter(ACTION_PORT_FORWARDING);
        mContext.registerReceiver(mReceiver, intentFilter, Context.RECEIVER_NOT_EXPORTED);
    }

    public void stop() {
        mPortsStateManager.unregisterListener(mPortsStateListener);
        mContext.unregisterReceiver(mReceiver);
    }

    private String getString(int resId) {
        return mContext.getString(resId);
    }

    private PendingIntent getPendingIntentFor(int port, boolean enabled) {
        Intent intent = new Intent(ACTION_PORT_FORWARDING);
        intent.setPackage(mContext.getPackageName());
        intent.setIdentifier(String.format(Locale.ROOT, "%d_%b", port, enabled));
        intent.putExtra(KEY_PORT, port);
        intent.putExtra(KEY_ENABLED, enabled);
        return PendingIntent.getBroadcast(mContext, 0, intent, PendingIntent.FLAG_IMMUTABLE);
    }

    private void showNotificationFor(int port) {
        Intent tapIntent = new Intent(mContext, SettingsPortForwardingActivity.class);
        tapIntent.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent tapPendingIntent =
                PendingIntent.getActivity(mContext, 0, tapIntent, PendingIntent.FLAG_IMMUTABLE);

        String title = getString(R.string.settings_port_forwarding_notification_title);
        String content =
                mContext.getString(R.string.settings_port_forwarding_notification_content, port);
        String acceptText = getString(R.string.settings_port_forwarding_notification_accept);
        String denyText = getString(R.string.settings_port_forwarding_notification_deny);
        Icon icon = Icon.createWithResource(mContext, R.drawable.ic_launcher_foreground);

        Action acceptAction =
                new Action.Builder(icon, acceptText, getPendingIntentFor(port, true /* enabled */))
                        .build();
        Action denyAction =
                new Action.Builder(icon, denyText, getPendingIntentFor(port, false /* enabled */))
                        .build();
        Notification notification =
                new Notification.Builder(mContext, mContext.getPackageName())
                        .setSmallIcon(R.drawable.ic_launcher_foreground)
                        .setContentTitle(title)
                        .setContentText(content)
                        .setContentIntent(tapPendingIntent)
                        .addAction(acceptAction)
                        .addAction(denyAction)
                        .build();
        mNotificationManager.notify(TAG, port, notification);
    }

    private void discardNotificationFor(int port) {
        mNotificationManager.cancel(TAG, port);
    }

    private final class PortForwardingRequestReceiver extends BroadcastReceiver {
        @Override
        public void onReceive(Context context, Intent intent) {
            if (ACTION_PORT_FORWARDING.equals(intent.getAction())) {
                performActionPortForwarding(context, intent);
            }
        }

        private void performActionPortForwarding(Context context, Intent intent) {
            int port = intent.getIntExtra(KEY_PORT, 0);
            boolean enabled = intent.getBooleanExtra(KEY_ENABLED, false);
            mPortsStateManager.updateEnabledPort(port, enabled);
            discardNotificationFor(port);
        }
    }
}
