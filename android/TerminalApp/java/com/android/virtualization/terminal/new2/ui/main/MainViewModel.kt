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
import android.content.Context
import android.content.Intent
import android.hardware.display.DisplayManager
import android.net.ConnectivityManager
import android.net.Network
import android.util.DisplayMetrics
import android.view.Display
import android.view.WindowInsets
import android.view.WindowManager
import android.view.WindowManager.LayoutParams.TYPE_APPLICATION
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.android.virtualization.terminal.DisplayInfo
import com.android.virtualization.terminal.new2.core.InstallState
import com.android.virtualization.terminal.new2.core.Installer
import com.android.virtualization.terminal.new2.core.TerminalSession
import com.android.virtualization.terminal.new2.core.VmController
import com.android.virtualization.terminal.new2.core.VmState
import com.android.virtualization.terminal.new2.ui.MainActivity
import com.android.virtualization.terminal.new2.ui.SettingsDestination
import com.android.virtualization.terminal.new2.ui.TAB_BAR_HEIGHT
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
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

    data class Running(val address: String, val port: Int, val key: String?) : MainUiState

    data object Stopping : MainUiState

    sealed interface ErrorHandler {
        data object Retry : ErrorHandler

        data object CheckNetwork : ErrorHandler

        data class ReportBug(val error: Throwable) : ErrorHandler
    }

    data class Error(val handler: ErrorHandler) : MainUiState
}

sealed interface DisplayState {
    data object Hidden : DisplayState

    data object Normal : DisplayState

    data object Minimized : DisplayState
}

class MainViewModel(application: Application) : AndroidViewModel(application) {
    var hasVmEverStarted = false

    private val _tabs = MutableStateFlow<List<TerminalSession>>(listOf(TerminalSession()))
    val tabs: StateFlow<List<TerminalSession>> = _tabs.asStateFlow()

    private val _selectedTabId = MutableStateFlow(_tabs.value.first().id)
    val selectedTabId: StateFlow<String> = _selectedTabId.asStateFlow()

    private val _displayState = MutableStateFlow<DisplayState>(DisplayState.Hidden)
    val displayState: StateFlow<DisplayState> = _displayState.asStateFlow()

    private val _isImeVisible = MutableStateFlow(false)
    val isImeVisible: StateFlow<Boolean> = _isImeVisible.asStateFlow()

    private val _isPanZoomMode = MutableStateFlow(false)
    val isPanZoomMode: StateFlow<Boolean> = _isPanZoomMode.asStateFlow()

    private val _isMouseLocked = MutableStateFlow(false)
    val isMouseLocked: StateFlow<Boolean> = _isMouseLocked.asStateFlow()

    private val _hasBackup = MutableStateFlow(false)
    val hasBackup: StateFlow<Boolean> = _hasBackup.asStateFlow()

    private val _settingsRequest = MutableSharedFlow<SettingsDestination?>()
    val settingsRequest: SharedFlow<SettingsDestination?> = _settingsRequest.asSharedFlow()

    fun handleIntent(intent: Intent) {
        if (intent.action == MainActivity.ACTION_OPEN_SETTINGS_PORT) {
            viewModelScope.launch { _settingsRequest.emit(SettingsDestination.PortControl) }
        }
    }

    fun toggleDisplay() {
        _displayState.value =
            if (_displayState.value == DisplayState.Hidden) {
                DisplayState.Normal
            } else {
                DisplayState.Hidden
            }
    }

    fun setIsImeVisible(visible: Boolean) {
        _isImeVisible.value = visible
    }

    fun setPanZoomMode(enabled: Boolean) {
        _isPanZoomMode.value = enabled
    }

    fun setMouseLocked(locked: Boolean) {
        _isMouseLocked.value = locked
    }

    private val connectivityManager = application.getSystemService(ConnectivityManager::class.java)
    private val networkCallback =
        object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                Installer.retryCheck()
            }
        }

    init {
        VmController.reset()
        connectivityManager?.registerDefaultNetworkCallback(networkCallback)

        viewModelScope.launch {
            Installer.installState.collect { state ->
                if (state is InstallState.Installed) {
                    try {
                        connectivityManager?.unregisterNetworkCallback(networkCallback)
                    } catch (e: IllegalArgumentException) {
                        // Already unregistered
                    }
                }
            }
        }
        viewModelScope.launch(Dispatchers.IO) { _hasBackup.value = Installer.hasBackup() }
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
        _displayState.value = DisplayState.Hidden
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
                            is VmState.Running ->
                                MainUiState.Running(vmState.address, vmState.port, vmState.key)
                            is VmState.Stopping -> MainUiState.Stopping
                            is VmState.Stopped -> {
                                if (hasVmEverStarted) {
                                    MainUiState.Stopped
                                } else {
                                    MainUiState.Ready
                                }
                            }
                            is VmState.Error -> {
                                setIsImeVisible(false)
                                MainUiState.Error(MainUiState.ErrorHandler.ReportBug(vmState.cause))
                            }
                        }
                    }
                    is InstallState.Error -> {
                        val handler =
                            when (installState.errorCause) {
                                InstallState.ErrorCause.CheckFailed ->
                                    MainUiState.ErrorHandler.CheckNetwork
                                InstallState.ErrorCause.InstallFailed ->
                                    MainUiState.ErrorHandler.Retry
                                InstallState.ErrorCause.UninstallFailed,
                                InstallState.ErrorCause.DeleteBackupFailed ->
                                    MainUiState.ErrorHandler.ReportBug(installState.cause)
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

    fun retryCheck() {
        Installer.retryCheck()
    }

    fun startVm() {
        val displayInfo = getDisplayInfo(getApplication())
        viewModelScope.launch { VmController.start(displayInfo) }
    }

    fun stopVm() {
        VmController.stop()
    }

    fun uninstallVm(backupRootfs: Boolean) {
        viewModelScope.launch {
            VmController.stop()
            Installer.uninstall(backupRootfs)
        }
    }

    fun deleteBackup() {
        viewModelScope.launch {
            if (Installer.deleteBackup()) {
                _hasBackup.value = false
                startVm()
            }
        }
    }

    override fun onCleared() {
        super.onCleared()
        VmController.stop()
        try {
            connectivityManager?.unregisterNetworkCallback(networkCallback)
        } catch (e: IllegalArgumentException) {
            // Already unregistered
        }
    }

    private fun getDisplayInfo(context: Context): DisplayInfo {
        val dm = context.getSystemService(DisplayManager::class.java)
        val display = dm.getDisplay(Display.DEFAULT_DISPLAY)
        val windowContext =
            context.createDisplayContext(display).createWindowContext(TYPE_APPLICATION, null)
        val wm = windowContext.getSystemService(WindowManager::class.java)
        val metrics = wm.currentWindowMetrics
        val insets = metrics.windowInsets.getInsets(WindowInsets.Type.systemBars())

        var width = metrics.bounds.width() - insets.left - insets.right
        val density = context.resources.displayMetrics.density
        var height =
            metrics.bounds.height() -
                insets.top -
                insets.bottom -
                (TAB_BAR_HEIGHT.value * density).toInt()

        val maxDim = 1280
        if (width > maxDim || height > maxDim) {
            if (width > height) {
                height = (height * maxDim.toFloat() / width).toInt()
                width = maxDim
            } else {
                width = (width * maxDim.toFloat() / height).toInt()
                height = maxDim
            }
        }

        val dpi = (DisplayMetrics.DENSITY_DEFAULT * density).toInt()
        val refreshRate = display.refreshRate.toInt()
        return DisplayInfo(width, height, dpi, refreshRate)
    }
}
