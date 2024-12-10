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

import android.content.Context;
import android.content.SharedPreferences;

import com.android.internal.annotations.GuardedBy;

import java.util.HashSet;
import java.util.Set;
import java.util.stream.Collectors;

/**
 * PortsStateManager is responsible for communicating with shared preferences and managing state of
 * ports.
 */
public class PortsStateManager {
    private static final String PREFS_NAME = ".PORTS";
    private static final int FLAG_ENABLED = 1;

    private static PortsStateManager mInstance;
    private final Object mLock = new Object();

    private final SharedPreferences mSharedPref;

    @GuardedBy("mLock")
    private Set<Integer> mActivePorts;

    @GuardedBy("mLock")
    private final Set<Integer> mEnabledPorts;

    @GuardedBy("mLock")
    private final Set<Listener> mListeners;

    private PortsStateManager(SharedPreferences sharedPref) {
        mSharedPref = sharedPref;
        mEnabledPorts =
                mSharedPref.getAll().entrySet().stream()
                        .filter(entry -> entry.getValue() instanceof Integer)
                        .filter(entry -> ((int) entry.getValue() & FLAG_ENABLED) == FLAG_ENABLED)
                        .map(entry -> entry.getKey())
                        .filter(
                                key -> {
                                    try {
                                        Integer.parseInt(key);
                                        return true;
                                    } catch (NumberFormatException e) {
                                        return false;
                                    }
                                })
                        .map(Integer::parseInt)
                        .collect(Collectors.toSet());
        mActivePorts = new HashSet<>();
        mListeners = new HashSet<>();
    }

    static synchronized PortsStateManager getInstance(Context context) {
        if (mInstance == null) {
            SharedPreferences sharedPref =
                    context.getSharedPreferences(
                            context.getPackageName() + PREFS_NAME, Context.MODE_PRIVATE);
            mInstance = new PortsStateManager(sharedPref);
        }
        return mInstance;
    }

    Set<Integer> getActivePorts() {
        synchronized (mLock) {
            return new HashSet<>(mActivePorts);
        }
    }

    Set<Integer> getEnabledPorts() {
        synchronized (mLock) {
            return new HashSet<>(mEnabledPorts);
        }
    }

    void updateActivePorts(Set<Integer> ports) {
        Set<Integer> oldPorts;
        synchronized (mLock) {
            oldPorts = mActivePorts;
            mActivePorts = ports;
        }
        notifyPortsStateUpdated(oldPorts, ports);
    }

    void updateEnabledPort(int port, boolean enabled) {
        Set<Integer> activePorts;
        synchronized (mLock) {
            SharedPreferences.Editor editor = mSharedPref.edit();
            editor.putInt(String.valueOf(port), enabled ? FLAG_ENABLED : 0);
            editor.apply();
            if (enabled) {
                mEnabledPorts.add(port);
            } else {
                mEnabledPorts.remove(port);
            }
            activePorts = mActivePorts;
        }
        notifyPortsStateUpdated(activePorts, activePorts);
    }

    void registerListener(Listener listener) {
        synchronized (mLock) {
            mListeners.add(listener);
        }
    }

    void unregisterListener(Listener listener) {
        synchronized (mLock) {
            mListeners.remove(listener);
        }
    }

    private void notifyPortsStateUpdated(Set<Integer> oldActivePorts, Set<Integer> newActivePorts) {
        Set<Listener> listeners;
        synchronized (mLock) {
            listeners = new HashSet<>(mListeners);
        }
        for (Listener listener : listeners) {
            listener.onPortsStateUpdated(
                    new HashSet<>(oldActivePorts), new HashSet<>(newActivePorts));
        }
    }

    interface Listener {
        default void onPortsStateUpdated(
                Set<Integer> oldActivePorts, Set<Integer> newActivePorts) {}
    }
}
