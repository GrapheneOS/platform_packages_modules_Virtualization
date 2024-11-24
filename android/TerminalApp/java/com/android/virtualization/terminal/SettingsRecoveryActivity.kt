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
import android.view.View
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.isVisible
import androidx.lifecycle.lifecycleScope
import com.google.android.material.card.MaterialCardView
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.snackbar.Snackbar
import java.io.IOException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

const val TAG: String = "VmTerminalApp"

class SettingsRecoveryActivity : AppCompatActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.settings_recovery)
        val resetCard = findViewById<MaterialCardView>(R.id.settings_recovery_reset_card)
        resetCard.setOnClickListener {
            var backupRootfs = false
            val dialog = MaterialAlertDialogBuilder(this)
                .setTitle(R.string.settings_recovery_reset_dialog_title)
                .setMultiChoiceItems(arrayOf(getString(R.string.settings_recovery_reset_dialog_backup_option)), booleanArrayOf(backupRootfs)) {_, _, checked ->
                    backupRootfs = checked
                }
                .setPositiveButton(R.string.settings_recovery_reset_dialog_confirm) { _, _ ->
                    // This coroutine will be killed when the activity is killed. The behavior is both acceptable
                    // either removing is done or not
                    runInBackgroundAndRestartApp {
                        uninstall(backupRootfs)
                    }
                }
                .setNegativeButton(R.string.settings_recovery_reset_dialog_cancel) { dialog, _ -> dialog.dismiss() }
                .create()
            dialog.show()
        }
        val resetBackupCard = findViewById<View>(R.id.settings_recovery_reset_backup_card)
        resetBackupCard.isVisible = InstallUtils.getBackupFile(this).exists()

        resetBackupCard.setOnClickListener {
            val dialog = MaterialAlertDialogBuilder(this)
                .setTitle(R.string.settings_recovery_remove_backup_title)
                .setMessage(R.string.settings_recovery_remove_backup_sub_title)
                .setPositiveButton(R.string.settings_recovery_reset_dialog_confirm) { _, _ ->
                    runInBackgroundAndRestartApp {
                        removeBackup()
                    }
                }
                .setNegativeButton(R.string.settings_recovery_reset_dialog_cancel) { dialog, _ -> dialog.dismiss() }
                .create()
            dialog.show()
        }
    }

    private fun removeBackup(): Unit {
        if (!InstallUtils.getBackupFile(this@SettingsRecoveryActivity).delete()) {
            Snackbar.make(
                findViewById(android.R.id.content),
                R.string.settings_recovery_error_during_removing_backup,
                Snackbar.LENGTH_SHORT
            ).show();
            Log.e(TAG, "cannot remove backup")
        }
    }

    private fun uninstall(backupRootfs: Boolean): Unit {
        var backupDone = false
        try {
            if (backupRootfs) {
                InstallUtils.backupRootFs(this@SettingsRecoveryActivity)
                backupDone = true
            }
            InstallUtils.deleteInstallation(this@SettingsRecoveryActivity)
        } catch (e: IOException) {
            val errorMsgId = if (backupRootfs && !backupDone) R.string.settings_recovery_error_due_to_backup
                    else R.string.settings_recovery_error;
            Snackbar.make(
                findViewById(android.R.id.content),
                errorMsgId,
                Snackbar.LENGTH_SHORT
            ).show();
            Log.e(TAG, "cannot recovery ", e)
        }
    }

    private fun runInBackgroundAndRestartApp(backgroundWork: suspend CoroutineScope.() -> Unit): Unit {
        findViewById<View>(R.id.setting_recovery_card_container).visibility = View.INVISIBLE
        findViewById<View>(R.id.recovery_boot_progress).visibility = View.VISIBLE
        lifecycleScope.launch(Dispatchers.IO) {
            backgroundWork()
        }.invokeOnCompletion {
            runOnUiThread {
                findViewById<View>(R.id.setting_recovery_card_container).visibility =
                    View.VISIBLE
                findViewById<View>(R.id.recovery_boot_progress).visibility = View.INVISIBLE
                // Restart terminal
                val intent =
                    baseContext.packageManager.getLaunchIntentForPackage(baseContext.packageName)
                intent?.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK)
                finish()
                startActivity(intent)
            }
        }
    }
}