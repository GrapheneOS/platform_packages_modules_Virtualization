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
import android.net.ConnectivityManager
import android.net.Network
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.android.virtualization.terminal.new2.core.InstallState
import com.android.virtualization.terminal.new2.core.Installer
import com.android.virtualization.terminal.new2.core.TerminalSession
import com.android.virtualization.terminal.new2.core.VmController
import com.android.virtualization.terminal.new2.core.VmState
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
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

    sealed interface ErrorHandler {
        data object Retry : ErrorHandler

        data object CheckNetwork : ErrorHandler

        data class ReportBug(val error: Throwable) : ErrorHandler
    }

    data class Error(val handler: ErrorHandler) : MainUiState
}

class MainViewModel(application: Application) : AndroidViewModel(application) {
    var hasVmEverStarted = false

    private val _tabs = MutableStateFlow<List<TerminalSession>>(listOf(TerminalSession()))
    val tabs: StateFlow<List<TerminalSession>> = _tabs.asStateFlow()

    private val _selectedTabId = MutableStateFlow(_tabs.value.first().id)
    val selectedTabId: StateFlow<String> = _selectedTabId.asStateFlow()

    init {
        VmController.reset()
        val connectivityManager = application.getSystemService(ConnectivityManager::class.java)
        connectivityManager?.registerDefaultNetworkCallback(
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    Installer.retryCheck()
                }
            }
        )
    }

    fun addTab() {
        val newSession = TerminalSession()
        val currentTabs = _tabs.value.toMutableList()
        currentTabs.add(newSession)
        _tabs.value = currentTabs
        _selectedTabId.value = newSession.id
    }

    fun closeTab(id: String) {
        val currentTabs = _tabs.value.toMutableList()

        val index = currentTabs.indexOfFirst { it.id == id }
        if (index == -1) return

        currentTabs.removeAt(index)
        _tabs.value = currentTabs

        if (currentTabs.isEmpty()) {
            stopVm()
            return
        }

        if (_selectedTabId.value == id) {
            // Select the previous tab, or the first one if we closed the first
            val newIndex = if (index > 0) index - 1 else 0
            _selectedTabId.value = currentTabs[newIndex].id
        }
    }

    fun selectTab(id: String) {
        _selectedTabId.value = id
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
                                MainUiState.Error(MainUiState.ErrorHandler.ReportBug(vmState.cause))
                        }
                    }
                    is InstallState.Error -> {
                        val handler =
                            when (installState.errorCause) {
                                InstallState.ErrorCause.CheckFailed ->
                                    MainUiState.ErrorHandler.CheckNetwork
                                InstallState.ErrorCause.InstallFailed ->
                                    MainUiState.ErrorHandler.Retry
                            }
                        MainUiState.Error(handler)
                    }
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
