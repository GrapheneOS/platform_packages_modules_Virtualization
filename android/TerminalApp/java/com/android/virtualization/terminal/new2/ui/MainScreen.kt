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
package com.android.virtualization.terminal.new2.ui

import android.app.Activity
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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.android.virtualization.terminal.BetterBugLauncher
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.core.InstallState
import com.android.virtualization.terminal.new2.core.Installer
import com.android.virtualization.terminal.new2.ui.main.DisplayState
import com.android.virtualization.terminal.new2.ui.main.MainUiState
import com.android.virtualization.terminal.new2.ui.main.MainViewModel

@Composable
fun MainScreen(viewModel: MainViewModel) {
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    val installState by Installer.installState.collectAsStateWithLifecycle()
    val showSettings by viewModel.showSettings.collectAsStateWithLifecycle()

    var lastValidState by remember { mutableStateOf<MainUiState>(MainUiState.Ready) }
    if (uiState !is MainUiState.Error) {
        lastValidState = uiState
    }

    val snackbarHostState = remember { SnackbarHostState() }
    val context = LocalContext.current
    val activity = context as Activity

    PermissionChecker(viewModel, snackbarHostState)

    LaunchedEffect(uiState) {
        when (val state = uiState) {
            is MainUiState.Ready -> {
                snackbarHostState.currentSnackbarData?.dismiss()
            }
            is MainUiState.Stopped -> activity.finish()
            is MainUiState.Error -> {
                handleError(activity, snackbarHostState, state.handler)
            }
            else -> {}
        }
    }

    Scaffold(snackbarHost = { SnackbarHost(hostState = snackbarHostState) }) { innerPadding ->
        val padding = innerPadding

        Box(modifier = Modifier.fillMaxSize()) {
            Box(modifier = Modifier.padding(padding).fillMaxSize()) {
                if (installState !is InstallState.Installed) {
                    InstallScreen(snackbarHostState = snackbarHostState)
                } else
                    when (val state = lastValidState) {
                        is MainUiState.Ready -> {
                            // VM will soon be booting
                        }
                        is MainUiState.Stopped -> {
                            // Activity will finish
                        }
                        is MainUiState.Booting -> BootingScreen()
                        is MainUiState.Running -> RunningScreen(state, viewModel)
                        is MainUiState.Stopping -> BootingScreen() // TODO: show the shutdown screen
                        else -> {}
                    }
            }

            AnimatedVisibility(
                visible = showSettings,
                enter = slideInHorizontally(initialOffsetX = { it }),
                exit = slideOutHorizontally(targetOffsetX = { it }),
            ) {
                SettingsScreen(onBack = { viewModel.setShowSettings(false) })
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

@Composable
fun RunningScreen(state: MainUiState.Running, viewModel: MainViewModel) {
    val tabs by viewModel.tabs.collectAsStateWithLifecycle()
    val selectedTabId by viewModel.selectedTabId.collectAsStateWithLifecycle()
    val displayState by viewModel.displayState.collectAsStateWithLifecycle()

    Column {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface),
        ) {
            Box(modifier = Modifier.weight(1f)) {
                TerminalTabBar(
                    tabs = tabs,
                    selectedTabId =
                        if (displayState == DisplayState.Normal) null else selectedTabId,
                    onTabSelected = { viewModel.selectTab(it) },
                    onTabClosed = { viewModel.closeTab(it) },
                    onAddTab = { viewModel.addTab() },
                )
            }
            DisplayController(viewModel = viewModel)
            IconButton(
                onClick = {
                    viewModel.setShowSettings(true)
                    viewModel.setIsImeVisible(false)
                }
            ) {
                Icon(Icons.Default.Settings, contentDescription = "Settings")
            }
        }
        if (displayState == DisplayState.Normal) {
            Box(modifier = Modifier.fillMaxSize()) { DisplayScreen(viewModel = viewModel) }
        } else {
            TerminalScreen(state.terminalAddress, selectedTabId, viewModel)
        }
    }
}

private suspend fun handleError(
    activity: Activity,
    snackbarHostState: SnackbarHostState,
    handler: MainUiState.ErrorHandler,
) {
    val (messageId, actionLabel, action) =
        when (handler) {
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
