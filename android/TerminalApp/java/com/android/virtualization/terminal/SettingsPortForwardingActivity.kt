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

import android.content.DialogInterface
import android.os.Bundle
import android.text.Editable
import android.text.TextWatcher
import android.widget.EditText
import android.widget.ImageButton
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.dialog.MaterialAlertDialogBuilder

private const val PORT_RANGE_MIN: Int = 1024
private const val PORT_RANGE_MAX: Int = 65535

class SettingsPortForwardingActivity : AppCompatActivity() {
    private lateinit var mPortsStateManager: PortsStateManager
    private lateinit var mPortsStateListener: Listener
    private lateinit var mActivePortsAdapter: SettingsPortForwardingActiveAdapter
    private lateinit var mInactivePortsAdapter: SettingsPortForwardingInactiveAdapter

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.settings_port_forwarding)

        mPortsStateManager = PortsStateManager.getInstance(this)

        mActivePortsAdapter = SettingsPortForwardingActiveAdapter(mPortsStateManager, this)
        val activeRecyclerView: RecyclerView =
            findViewById(R.id.settings_port_forwarding_active_recycler_view)
        activeRecyclerView.layoutManager = LinearLayoutManager(this)
        activeRecyclerView.adapter = mActivePortsAdapter

        mInactivePortsAdapter = SettingsPortForwardingInactiveAdapter(mPortsStateManager, this)
        val inactiveRecyclerView: RecyclerView =
            findViewById(R.id.settings_port_forwarding_inactive_recycler_view)
        inactiveRecyclerView.layoutManager = LinearLayoutManager(this)
        inactiveRecyclerView.adapter = mInactivePortsAdapter

        mPortsStateListener = Listener()

        val addButton = findViewById<ImageButton>(R.id.settings_port_forwarding_inactive_add_button)
        addButton.setOnClickListener {
            val dialog =
                MaterialAlertDialogBuilder(this)
                    .setTitle(R.string.settings_port_forwarding_dialog_title)
                    .setView(R.layout.settings_port_forwarding_inactive_add_dialog)
                    .setPositiveButton(R.string.settings_port_forwarding_dialog_save) {
                        dialogInterface,
                        _ ->
                        val alertDialog = dialogInterface as AlertDialog
                        val editText =
                            alertDialog.findViewById<EditText>(
                                R.id.settings_port_forwarding_inactive_add_dialog_text
                            )!!
                        val port = editText.text.toString().toInt()
                        mPortsStateManager.updateEnabledPort(port, true)
                    }
                    .setNegativeButton(R.string.settings_port_forwarding_dialog_cancel, null)
                    .create()
            dialog.show()

            val positiveButton = dialog.getButton(DialogInterface.BUTTON_POSITIVE)
            positiveButton.setEnabled(false)
            val editText =
                dialog.findViewById<EditText>(
                    R.id.settings_port_forwarding_inactive_add_dialog_text
                )!!
            editText.addTextChangedListener(
                object : TextWatcher {
                    override fun beforeTextChanged(
                        s: CharSequence?,
                        start: Int,
                        count: Int,
                        after: Int,
                    ) {}

                    override fun afterTextChanged(s: Editable?) {}

                    override fun onTextChanged(
                        s: CharSequence?,
                        start: Int,
                        before: Int,
                        count: Int,
                    ) {
                        val port =
                            try {
                                s.toString().toInt()
                            } catch (e: NumberFormatException) {
                                editText.setError(
                                    getString(
                                        R.string.settings_port_forwarding_dialog_error_invalid_input
                                    )
                                )
                                positiveButton.setEnabled(false)
                                return@onTextChanged
                            }
                        if (port > PORT_RANGE_MAX || port < PORT_RANGE_MIN) {
                            editText.setError(
                                getString(
                                    R.string
                                        .settings_port_forwarding_dialog_error_invalid_port_range
                                )
                            )
                            positiveButton.setEnabled(false)
                        } else if (
                            mPortsStateManager.getActivePorts().contains(port) ||
                                mPortsStateManager.getEnabledPorts().contains(port)
                        ) {
                            editText.setError(
                                getString(
                                    R.string.settings_port_forwarding_dialog_error_existing_port
                                )
                            )
                            positiveButton.setEnabled(false)
                        } else {
                            positiveButton.setEnabled(true)
                        }
                    }
                }
            )
        }
    }

    private fun refreshAdapters() {
        runOnUiThread {
            mActivePortsAdapter.refreshItems()
            mInactivePortsAdapter.refreshItems()
        }
    }

    override fun onResume() {
        super.onResume()
        mPortsStateManager.registerListener(mPortsStateListener)
        refreshAdapters()
    }

    override fun onPause() {
        mPortsStateManager.unregisterListener(mPortsStateListener)
        super.onPause()
    }

    private inner class Listener : PortsStateManager.Listener {
        override fun onPortsStateUpdated(oldActivePorts: Set<Int>, newActivePorts: Set<Int>) {
            refreshAdapters()
        }
    }
}
