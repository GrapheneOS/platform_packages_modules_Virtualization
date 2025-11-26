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
package com.android.virtualization.terminal.new2.ui.main

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.android.virtualization.terminal.new2.core.InstallState
import com.android.virtualization.terminal.new2.core.Installer
import com.android.virtualization.terminal.new2.core.VmController
import com.android.virtualization.terminal.new2.core.VmState
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

sealed interface MainUiState {
    data object Checking : MainUiState

    data class NotInstalled(val totalSizeBytes: Long) : MainUiState

    data class Installing(val progress: StateFlow<Long>, val totalBytes: Long) : MainUiState

    data object Ready : MainUiState

    data object Stopped : MainUiState

    data object Booting : MainUiState

    data class Running(val address: String, val port: Int) : MainUiState

    data object Stopping : MainUiState

    data class Error(val message: String) : MainUiState
}

class MainViewModel(application: Application) : AndroidViewModel(application) {
    var hasVmEverStarted = false

    init {
        VmController.reset()
    }

    val uiState: StateFlow<MainUiState> =
        combine(Installer.installState, VmController.vmState) { installState, vmState ->
                when (installState) {
                    is InstallState.Checking -> MainUiState.Checking
                    is InstallState.NotInstalled ->
                        MainUiState.NotInstalled(installState.totalSizeBytes)
                    is InstallState.Installing ->
                        MainUiState.Installing(installState.progress, installState.totalBytes)
                    is InstallState.Installed -> {
                        when (vmState) {
                            is VmState.Ready -> MainUiState.Ready
                            is VmState.Starting -> {
                                hasVmEverStarted = true
                                MainUiState.Booting
                            }
                            is VmState.Running -> MainUiState.Running(vmState.address, vmState.port)
                            is VmState.Stopping -> MainUiState.Stopping
                            is VmState.Stopped -> {
                                if (hasVmEverStarted) {
                                    MainUiState.Stopped
                                } else {
                                    MainUiState.Ready
                                }
                            }
                            is VmState.Error ->
                                MainUiState.Error(vmState.cause.message ?: "Unknown VM error")
                        }
                    }
                    is InstallState.Error ->
                        MainUiState.Error(installState.cause.message ?: "Unknown install error")
                }
            }
            .stateIn(
                scope = viewModelScope,
                started = SharingStarted.WhileSubscribed(5000),
                initialValue = MainUiState.Checking,
            )

    fun installVm() {
        viewModelScope.launch { Installer.install() }
    }

    fun cancelInstallVm() {
        Installer.cancelInstall()
    }

    fun startVm() {
        viewModelScope.launch { VmController.start() }
    }

    fun stopVm() {
        VmController.stop()
    }

    override fun onCleared() {
        super.onCleared()
        VmController.stop()
    }
}
