/*
 * Copyright (C) 2025 The Android Open Source Project
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
package com.android.virtualization.terminal.new2.core

import android.content.Context
import android.content.Intent
import android.util.Log
import com.android.virtualization.terminal.ImageArchive
import com.android.virtualization.terminal.InstalledImage
import com.android.virtualization.terminal.new2.util.LoggingMutableStateFlow
import java.io.IOException
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

object Installer {

    private lateinit var context: Context
    private val repositoryScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private lateinit var installedImage: InstalledImage
    private var installJob: Job? = null
    private val _installState =
        LoggingMutableStateFlow<InstallState>(MutableStateFlow(InstallState.Checking), "Installer")
    val installState: StateFlow<InstallState> = _installState.asStateFlow()

    fun initialize(context: Context) {
        this.context = context.applicationContext
        this.installedImage = InstalledImage.getDefault(this.context)
        repositoryScope.launch { checkInstallStatus() }
    }

    private suspend fun checkInstallStatus() {
        _installState.value = InstallState.Checking
        if (installedImage.isInstalled()) {
            _installState.value = InstallState.Installed
            return
        }

        var retryCount = 0
        while (true) {
            try {
                val downloadSize = ImageArchive.getDefault().getSize()
                _installState.value = InstallState.NotInstalled(downloadSize)
                return
            } catch (e: IOException) {
                if (retryCount >= 2) {
                    Log.e("Installer", "Failed to check install status", e)
                    _installState.value = InstallState.Error(e, InstallState.ErrorCause.CheckFailed)
                    return
                }
                retryCount++
                delay(1000)
            }
        }
    }

    fun install() {
        val intent = Intent(context, VmService::class.java)
        context.startForegroundService(intent)
        installJob =
            repositoryScope.launch {
                val progressFlow = MutableStateFlow(0L)
                _installState.value =
                    InstallState.Installing(0L, progressFlow.asStateFlow()) // Initial state
                try {
                    val archive = ImageArchive.getDefault()
                    val totalSize = archive.getSize()
                    _installState.value =
                        InstallState.Installing(totalSize, progressFlow.asStateFlow())
                    archive.installTo(installedImage.installDir, null).collect { bytesRead ->
                        progressFlow.value = bytesRead
                    }
                    _installState.value = InstallState.Installed
                } catch (e: IOException) {
                    _installState.value =
                        InstallState.Error(e, InstallState.ErrorCause.InstallFailed)
                } catch (e: CancellationException) {
                    checkInstallStatus()
                }
            }
    }

    fun cancelInstall() {
        installJob?.cancel()
    }

    fun retryCheck() {
        val state = _installState.value
        if (state is InstallState.Error) {
            repositoryScope.launch { checkInstallStatus() }
        }
    }

    suspend fun uninstall(backupRootfs: Boolean) {
        withContext(Dispatchers.IO) {
            try {
                _installState.value = InstallState.Checking
                if (backupRootfs) {
                    installedImage.uninstallAndBackup()
                } else {
                    installedImage.uninstallFully()
                }
                checkInstallStatus()
            } catch (e: IOException) {
                Log.e("Installer", "Failed to uninstall or backup VM", e)
                _installState.value = InstallState.Error(e, InstallState.ErrorCause.UninstallFailed)
            }
        }
    }

    fun hasBackup(): Boolean {
        return installedImage.hasBackup()
    }

    suspend fun deleteBackup(): Boolean {
        return withContext(Dispatchers.IO) {
            try {
                installedImage.deleteBackup()
                true
            } catch (e: IOException) {
                Log.e("Installer", "Failed to delete backup", e)
                _installState.value =
                    InstallState.Error(e, InstallState.ErrorCause.DeleteBackupFailed)
                false
            }
        }
    }
}
