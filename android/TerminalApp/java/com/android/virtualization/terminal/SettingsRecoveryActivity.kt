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

import android.content.Intent
import android.os.Bundle
import android.util.Log
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.lifecycleScope
import com.android.virtualization.vmlauncher.InstallUtils
import com.google.android.material.card.MaterialCardView
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import java.io.IOException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

const val TAG: String = "VmTerminalApp"

class SettingsRecoveryActivity : AppCompatActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.settings_recovery)
        val resetCard = findViewById<MaterialCardView>(R.id.settings_recovery_reset_card)
        val dialog = MaterialAlertDialogBuilder(this)
            .setTitle(R.string.settings_recovery_reset_dialog_title)
            .setMessage(R.string.settings_recovery_reset_dialog_message)
            .setPositiveButton(R.string.settings_recovery_reset_dialog_confirm) { _, _ ->
                // This coroutine will be killed when the activity is killed. The behavior is both acceptable
                // either removing is done or not
                lifecycleScope.launch(Dispatchers.IO) {
                    try {
                        InstallUtils.unInstall(this@SettingsRecoveryActivity)
                        // Restart terminal
                        val intent =
                            baseContext.packageManager.getLaunchIntentForPackage(baseContext.packageName)
                        intent?.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK)
                        finish()
                        startActivity(intent)
                    } catch (e: IOException) {
                        Log.e(TAG, "VM image reset failed.")
                    }
                }
            }
            .setNegativeButton(R.string.settings_recovery_reset_dialog_cancel) { dialog, _ -> dialog.dismiss() }
            .create()
        resetCard.setOnClickListener {
            dialog.show()
        }
    }
}