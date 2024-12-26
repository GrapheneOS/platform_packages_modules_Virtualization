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

import android.content.Context
import android.content.SharedPreferences
import com.android.internal.annotations.GuardedBy
import java.util.HashSet

/**
 * PortsStateManager is responsible for communicating with shared preferences and managing state of
 * ports.
 */
class PortsStateManager private constructor(private val sharedPref: SharedPreferences) {
    private val lock = Any()

    @GuardedBy("lock") private var activePorts: MutableSet<Int> = hashSetOf()

    @GuardedBy("lock")
    private val enabledPorts: MutableSet<Int> =
        sharedPref
            .getAll()
            .entries
            .filterIsInstance<MutableMap.MutableEntry<String, Int>>()
            .filter { it.value and FLAG_ENABLED == FLAG_ENABLED }
            .map { it.key.toIntOrNull() }
            .filterNotNull()
            .toMutableSet()

    @GuardedBy("lock") private val listeners: MutableSet<Listener> = hashSetOf()

    fun getActivePorts(): MutableSet<Int> {
        synchronized(lock) {
            return HashSet<Int>(activePorts)
        }
    }

    fun getEnabledPorts(): MutableSet<Int> {
        synchronized(lock) {
            return HashSet<Int>(enabledPorts)
        }
    }

    fun updateActivePorts(ports: MutableSet<Int>) {
        var oldPorts: MutableSet<Int>
        synchronized(lock) {
            oldPorts = activePorts
            activePorts = ports
        }
        notifyPortsStateUpdated(oldPorts, ports)
    }

    fun updateEnabledPort(port: Int, enabled: Boolean) {
        var activePorts: MutableSet<Int>
        synchronized(lock) {
            val editor = sharedPref.edit()
            editor.putInt(port.toString(), if (enabled) FLAG_ENABLED else 0)
            editor.apply()
            if (enabled) {
                enabledPorts.add(port)
            } else {
                enabledPorts.remove(port)
            }
            activePorts = this@PortsStateManager.activePorts
        }
        notifyPortsStateUpdated(activePorts, activePorts)
    }

    fun clearEnabledPorts() {
        var activePorts: MutableSet<Int>
        synchronized(lock) {
            val editor = sharedPref.edit()
            editor.clear()
            editor.apply()
            enabledPorts.clear()
            activePorts = this@PortsStateManager.activePorts
        }
        notifyPortsStateUpdated(activePorts, activePorts)
    }

    fun registerListener(listener: Listener) {
        synchronized(lock) { listeners.add(listener) }
    }

    fun unregisterListener(listener: Listener) {
        synchronized(lock) { listeners.remove(listener) }
    }

    private fun notifyPortsStateUpdated(
        oldActivePorts: MutableSet<Int>,
        newActivePorts: MutableSet<Int>,
    ) {
        var listeners: MutableSet<Listener>
        synchronized(lock) { listeners = HashSet<Listener>(this@PortsStateManager.listeners) }
        for (listener in listeners) {
            listener.onPortsStateUpdated(HashSet<Int>(oldActivePorts), HashSet<Int>(newActivePorts))
        }
    }

    interface Listener {
        fun onPortsStateUpdated(oldActivePorts: Set<Int>, newActivePorts: Set<Int>) {}
    }

    companion object {
        private const val PREFS_NAME = ".PORTS"
        private const val FLAG_ENABLED = 1

        private var instance: PortsStateManager? = null

        @JvmStatic
        @Synchronized
        fun getInstance(context: Context): PortsStateManager {
            if (instance == null) {
                val sharedPref =
                    context.getSharedPreferences(
                        context.getPackageName() + PREFS_NAME,
                        Context.MODE_PRIVATE,
                    )
                instance = PortsStateManager(sharedPref)
            }
            return instance!!
        }
    }
}
