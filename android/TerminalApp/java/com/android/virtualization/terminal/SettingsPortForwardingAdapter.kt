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

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.recyclerview.widget.RecyclerView
import androidx.recyclerview.widget.SortedList
import androidx.recyclerview.widget.SortedListAdapterCallback
import com.google.android.material.materialswitch.MaterialSwitch

class SettingsPortForwardingAdapter(private val mPortsStateManager: PortsStateManager) :
    RecyclerView.Adapter<SettingsPortForwardingAdapter.ViewHolder>() {

    private var mItems: SortedList<SettingsPortForwardingItem>
    private val mPortsStateListener: Listener

    init {
        mItems =
            SortedList(
                SettingsPortForwardingItem::class.java,
                object : SortedListAdapterCallback<SettingsPortForwardingItem>(this) {
                    override fun compare(
                        o1: SettingsPortForwardingItem,
                        o2: SettingsPortForwardingItem,
                    ): Int {
                        return o1.port - o2.port
                    }

                    override fun areContentsTheSame(
                        o1: SettingsPortForwardingItem,
                        o2: SettingsPortForwardingItem,
                    ): Boolean {
                        return o1.port == o2.port && o1.enabled == o2.enabled
                    }

                    override fun areItemsTheSame(
                        o1: SettingsPortForwardingItem,
                        o2: SettingsPortForwardingItem,
                    ): Boolean {
                        return o1.port == o2.port
                    }
                },
            )
        mItems.addAll(getCurrentSettingsPortForwardingItem())
        mPortsStateListener = Listener()
    }

    fun registerPortsStateListener() {
        mPortsStateManager.registerListener(mPortsStateListener)
        mItems.replaceAll(getCurrentSettingsPortForwardingItem())
    }

    fun unregisterPortsStateListener() {
        mPortsStateManager.unregisterListener(mPortsStateListener)
    }

    private fun getCurrentSettingsPortForwardingItem(): ArrayList<SettingsPortForwardingItem> {
        val enabledPorts = mPortsStateManager.getEnabledPorts()
        return mPortsStateManager
            .getActivePorts()
            .map { SettingsPortForwardingItem(it, enabledPorts.contains(it)) }
            .toCollection(ArrayList())
    }

    class ViewHolder(view: View) : RecyclerView.ViewHolder(view) {
        val enabledSwitch: MaterialSwitch =
            view.findViewById(R.id.settings_port_forwarding_item_enabled_switch)
        val port: TextView = view.findViewById(R.id.settings_port_forwarding_item_port)
    }

    override fun onCreateViewHolder(viewGroup: ViewGroup, viewType: Int): ViewHolder {
        val view =
            LayoutInflater.from(viewGroup.context)
                .inflate(R.layout.settings_port_forwarding_item, viewGroup, false)
        return ViewHolder(view)
    }

    override fun onBindViewHolder(viewHolder: ViewHolder, position: Int) {
        val port = mItems[position].port
        viewHolder.port.text = port.toString()
        viewHolder.enabledSwitch.contentDescription = viewHolder.port.text
        viewHolder.enabledSwitch.isChecked = mItems[position].enabled
        viewHolder.enabledSwitch.setOnCheckedChangeListener { _, isChecked ->
            mPortsStateManager.updateEnabledPort(port, isChecked)
        }
    }

    override fun getItemCount() = mItems.size()

    private inner class Listener : PortsStateManager.Listener {
        override fun onPortsStateUpdated(oldActivePorts: Set<Int>, newActivePorts: Set<Int>) {
            mItems.replaceAll(getCurrentSettingsPortForwardingItem())
        }
    }
}
