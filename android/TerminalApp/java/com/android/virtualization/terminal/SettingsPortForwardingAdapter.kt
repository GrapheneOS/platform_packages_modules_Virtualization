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
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.recyclerview.widget.RecyclerView
import androidx.recyclerview.widget.SortedList
import androidx.recyclerview.widget.SortedListAdapterCallback
import com.google.android.material.materialswitch.MaterialSwitch

class SettingsPortForwardingAdapter(
    private val sharedPref: SharedPreferences?,
    private val context: Context,
) :
    RecyclerView.Adapter<SettingsPortForwardingAdapter.ViewHolder>(),
    SharedPreferences.OnSharedPreferenceChangeListener {

    private var mItems: SortedList<SettingsPortForwardingItem>

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
    }

    private fun getCurrentSettingsPortForwardingItem(): ArrayList<SettingsPortForwardingItem> {
        val items = ArrayList<SettingsPortForwardingItem>()
        val ports =
            sharedPref!!.getStringSet(
                context.getString(R.string.preference_forwarding_ports),
                HashSet<String>(),
            )
        for (port in ports!!) {
            val enabled =
                sharedPref.getBoolean(
                    context.getString(R.string.preference_forwarding_port_is_enabled) + port,
                    false,
                )
            items.add(SettingsPortForwardingItem(port.toInt(), enabled))
        }
        return items
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
        viewHolder.port.text = mItems[position].port.toString()
        viewHolder.enabledSwitch.contentDescription = viewHolder.port.text
        viewHolder.enabledSwitch.isChecked = mItems[position].enabled
        viewHolder.enabledSwitch.setOnCheckedChangeListener { _, isChecked ->
            val sharedPref: SharedPreferences =
                context.getSharedPreferences(
                    context.getString(R.string.preference_file_key),
                    Context.MODE_PRIVATE,
                )
            val editor = sharedPref.edit()
            editor.putBoolean(
                context.getString(R.string.preference_forwarding_port_is_enabled) +
                    viewHolder.port.text,
                isChecked,
            )
            editor.apply()
        }
    }

    override fun getItemCount() = mItems.size()

    override fun onSharedPreferenceChanged(sharedPreferences: SharedPreferences?, key: String?) {
        if (
            key == context.getString(R.string.preference_forwarding_ports) ||
                key!!.startsWith(context.getString(R.string.preference_forwarding_port_is_enabled))
        ) {
            mItems.replaceAll(getCurrentSettingsPortForwardingItem())
        }
    }
}
