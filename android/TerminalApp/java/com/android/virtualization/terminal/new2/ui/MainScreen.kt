/*
 * Copyright (C) 2025 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.android.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package com.android.virtualization.terminal.new2.ui

import android.app.Activity
import android.content.Intent
import android.provider.Settings
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.android.virtualization.terminal.BetterBugLauncher
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.ui.main.DisplayState
import com.android.virtualization.terminal.new2.ui.main.MainUiState
import com.android.virtualization.terminal.new2.ui.main.MainViewModel

@Composable
fun MainScreen(viewModel: MainViewModel) {
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    val displayState by viewModel.displayState.collectAsStateWithLifecycle()
    var lastValidState by remember { mutableStateOf<MainUiState>(MainUiState.Checking) }
    if (uiState !is MainUiState.Error) {
        lastValidState = uiState
    }

    val tabs by viewModel.tabs.collectAsStateWithLifecycle()
    val selectedTabId by viewModel.selectedTabId.collectAsStateWithLifecycle()
    val snackbarHostState = remember { SnackbarHostState() }
    val context = LocalContext.current
    val activity = context as Activity
    val scope = rememberCoroutineScope()
    var showSettings by rememberSaveable { mutableStateOf(false) }
    var settingsInitialDestination by remember { mutableStateOf<SettingsDestination?>(null) }

    PermissionChecker(viewModel, snackbarHostState)

    LaunchedEffect(viewModel) {
        viewModel.settingsRequest.collect { destination ->
            if (destination != null) {
                settingsInitialDestination = destination
                showSettings = true
            }
        }
    }

    LaunchedEffect(uiState) {
        val state = uiState
        when (state) {
            is MainUiState.Ready -> {
                snackbarHostState.currentSnackbarData?.dismiss()
                viewModel.startVm()
            }
            is MainUiState.Stopped -> activity.finish()
            is MainUiState.NotInstalled,
            is MainUiState.Checking,
            is MainUiState.Booting -> showSettings = false
            is MainUiState.Error -> {
                showSettings = false
                handleError(activity, snackbarHostState, viewModel, state)
            }
            else -> {}
        }
    }

    Scaffold(snackbarHost = { SnackbarHost(hostState = snackbarHostState) }) { innerPadding ->
        val padding = innerPadding

        Box(modifier = Modifier.fillMaxSize()) {
            Box(modifier = Modifier.padding(padding).fillMaxSize()) {
                when (val state = lastValidState) {
                    is MainUiState.Checking -> SplashScreen()
                    is MainUiState.NotInstalled,
                    is MainUiState.Installing,
                    is MainUiState.InstallSuspended -> {
                        InstallScreen(viewModel)
                    }
                    is MainUiState.Ready -> {
                        // VM will soon be booting
                    }
                    is MainUiState.Stopped -> {
                        // Activity will finish
                    }
                    is MainUiState.Booting -> BootingScreen()
                    is MainUiState.Running -> {
                        val currentDisplayState = displayState

                        Column {
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                modifier =
                                    Modifier.fillMaxWidth()
                                        .background(MaterialTheme.colorScheme.surface),
                            ) {
                                Box(modifier = Modifier.weight(1f)) {
                                    TerminalTabBar(
                                        tabs = tabs,
                                        selectedTabId =
                                            if (currentDisplayState == DisplayState.Normal) null
                                            else selectedTabId,
                                        onTabSelected = { viewModel.selectTab(it) },
                                        onTabClosed = { viewModel.closeTab(it) },
                                        onAddTab = { viewModel.addTab() },
                                    )
                                }
                                DisplayController(viewModel = viewModel)
                                IconButton(
                                    onClick = {
                                        showSettings = true
                                        viewModel.setIsImeVisible(false)
                                    }
                                ) {
                                    Icon(Icons.Default.Settings, contentDescription = "Settings")
                                }
                            }
                            if (currentDisplayState == DisplayState.Normal) {
                                Box(modifier = Modifier.fillMaxSize()) {
                                    DisplayScreen(viewModel = viewModel)
                                }
                            } else if (selectedTabId != null) {
                                TerminalScreen(
                                    state.address,
                                    state.port,
                                    state.key,
                                    selectedTabId!!,
                                    viewModel,
                                )
                            }
                        }
                    }
                    is MainUiState.Stopping -> BootingScreen() // TODO: show the shutdown screen
                    else -> {}
                }
            }

            AnimatedVisibility(
                visible = showSettings,
                enter = slideInHorizontally(initialOffsetX = { it }),
                exit = slideOutHorizontally(targetOffsetX = { it }),
            ) {
                SettingsScreen(
                    onBack = { showSettings = false },
                    initialDestination = settingsInitialDestination,
                )
            }
        }
    }
}

@Composable
fun SplashScreen() {
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator()
    }
}

@Composable
fun BootingScreen() {
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator()
    }
}

private suspend fun handleError(
    activity: Activity,
    snackbarHostState: SnackbarHostState,
    viewModel: MainViewModel,
    state: MainUiState.Error,
) {
    val (messageId, actionLabel, action) =
        when (val handler = state.handler) {
            MainUiState.ErrorHandler.CheckNetwork ->
                Triple(
                    R.string.installer_snkbar_error_no_wifi,
                    activity.getString(R.string.action_settings),
                    { activity.startActivity(Intent(Settings.ACTION_WIFI_SETTINGS)) },
                )
            MainUiState.ErrorHandler.Retry ->
                Triple(
                    R.string.installer_snkbar_error_unknown,
                    activity.getString(R.string.notif_btn_retry),
                    { viewModel.retryCheck() },
                )
            MainUiState.ErrorHandler.NoSpace ->
                Triple(
                    R.string.installer_snkbar_error_no_space,
                    activity.getString(R.string.installer_snkbar_action_storage),
                    { activity.startActivity(Intent(Settings.ACTION_INTERNAL_STORAGE_SETTINGS)) },
                )
            is MainUiState.ErrorHandler.ReportBug ->
                Triple(
                    R.string.error_title,
                    activity.getString(R.string.error_btn_report_bug),
                    {
                        val error = handler.error
                        val exception = error as? Exception ?: Exception(error)
                        BetterBugLauncher.launchBetterBugActivity(activity, exception)
                    },
                )
        }

    val result =
        snackbarHostState.showSnackbar(
            message = activity.getString(messageId),
            actionLabel = actionLabel,
            duration = SnackbarDuration.Indefinite,
        )
    if (result == SnackbarResult.ActionPerformed) {
        action()
    }
}
