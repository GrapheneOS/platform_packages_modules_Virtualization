/*
 * Copyright (C) 2026 The Android Open Source Project
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
package com.android.virtualization.terminal.new2.ui.main

import android.app.ActivityManager
import android.app.Application
import android.content.Context
import androidx.lifecycle.AndroidViewModel
import com.android.virtualization.terminal.new2.core.Installer
import com.android.virtualization.terminal.new2.core.VmController
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

class SettingsViewModel(application: Application) : AndroidViewModel(application) {
    private val sharedPref = application.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    private val _currentMemoryMb =
        MutableStateFlow(sharedPref.getInt(KEY_MEMORY_MIB, DEFAULT_MEMORY_MIB))
    val currentMemoryMb: StateFlow<Int> = _currentMemoryMb.asStateFlow()

    val maxMemoryMb: Int = calculateMaxMemoryMb(application)

    private val _showRebootDialog = MutableStateFlow(false)
    val showRebootDialog: StateFlow<Boolean> = _showRebootDialog.asStateFlow()

    fun setMemoryMb(mb: Int) {
        if (mb != _currentMemoryMb.value) {
            sharedPref.edit().putInt(KEY_MEMORY_MIB, mb).apply()
            _currentMemoryMb.value = mb
            _showRebootDialog.value = true
        }
    }

    fun dismissRebootDialog() {
        _showRebootDialog.value = false
    }

    private fun calculateMaxMemoryMb(context: Context): Int {
        val activityManager = context.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
        val memoryInfo = ActivityManager.MemoryInfo()
        activityManager.getMemoryInfo(memoryInfo)
        // Set maximum to 70% of total system RAM
        return (memoryInfo.totalMem / (1024 * 1024) * 0.7).toInt()
    }

    // Recovery related operations moved from RecoveryPage
    suspend fun resetTerminal(backup: Boolean) {
        VmController.stop()
        Installer.uninstall(backup)
    }

    suspend fun deleteBackup(): Boolean {
        return Installer.deleteBackup()
    }

    fun hasBackup(): Boolean {
        return Installer.hasBackup()
    }

    companion object {
        internal const val PREFS_NAME = "terminal_settings"
        internal const val KEY_MEMORY_MIB = "memory_mib"
        const val DEFAULT_MEMORY_MIB = 1024
        const val MIN_MEMORY_MIB = 200
    }
}
