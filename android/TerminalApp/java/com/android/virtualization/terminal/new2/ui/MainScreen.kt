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
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.provider.Settings
import android.text.format.Formatter
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.android.virtualization.terminal.BetterBugLauncher
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.ui.main.MainUiState
import com.android.virtualization.terminal.new2.ui.main.MainViewModel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

@Composable
fun MainScreen(viewModel: MainViewModel) {
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    var lastValidState by remember { mutableStateOf<MainUiState>(MainUiState.Checking) }
    if (uiState !is MainUiState.Error) {
        lastValidState = uiState
    }

    val tabs by viewModel.tabs.collectAsStateWithLifecycle()
    val selectedTabId by viewModel.selectedTabId.collectAsStateWithLifecycle()
    val snackbarHostState = remember { SnackbarHostState() }
    val context = LocalContext.current
    val activity = context as? Activity
    val scope = rememberCoroutineScope()

    LaunchedEffect(uiState) {
        val state = uiState
        when (state) {
            is MainUiState.Ready -> viewModel.startVm()
            is MainUiState.Stopped -> activity?.finish()
            is MainUiState.Error ->
                handleError(context, activity, snackbarHostState, viewModel, state)
            else -> {}
        }
    }

    Scaffold(snackbarHost = { SnackbarHost(hostState = snackbarHostState) }) { innerPadding ->
        Box(modifier = Modifier.padding(innerPadding).fillMaxSize()) {
            when (val state = lastValidState) {
                is MainUiState.Checking -> SplashScreen()
                is MainUiState.NotInstalled ->
                    InstallStartScreen(
                        totalSizeBytes = state.totalSizeBytes,
                        onInstallClick = { wifiOnly ->
                            onInstallClick(context, scope, snackbarHostState, viewModel, wifiOnly)
                        },
                    )
                is MainUiState.Installing ->
                    InstallProgressScreen(
                        progressFlow = state.progress,
                        totalBytes = state.totalBytes,
                        onCancelClick = { viewModel.cancelInstallVm() },
                    )
                is MainUiState.Ready -> {
                    // VM will soon be booting
                }
                is MainUiState.Stopped -> {
                    // Activity will finish
                }
                is MainUiState.Booting -> BootingScreen()
                is MainUiState.Running -> {
                    val isDisplayActive by viewModel.isDisplayActive.collectAsStateWithLifecycle()
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
                                    selectedTabId = if (isDisplayActive) null else selectedTabId,
                                    onTabSelected = {
                                        viewModel.showDisplay(false)
                                        viewModel.selectTab(it)
                                    },
                                    onTabClosed = { viewModel.closeTab(it) },
                                    onAddTab = { viewModel.addTab() },
                                )
                            }
                            DisplayController(
                                isDisplayActive = isDisplayActive,
                                onDisplayToggle = { viewModel.showDisplay(!isDisplayActive) },
                            )
                        }
                        if (isDisplayActive) {
                            DisplayScreen()
                        } else if (selectedTabId != null) {
                            TerminalScreen(state.address, state.port, selectedTabId!!)
                        }
                    }
                }
                is MainUiState.Stopping -> BootingScreen() // TODO: show the shutdown screen
                else -> {}
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
fun InstallStartScreen(totalSizeBytes: Long, onInstallClick: (Boolean) -> Unit) {
    var wifiOnly by remember { mutableStateOf(true) }
    val context = LocalContext.current
    val formattedSize = Formatter.formatFileSize(context, totalSizeBytes)

    Column(
        modifier = Modifier.fillMaxSize().padding(16.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(text = stringResource(R.string.installer_desc_text_format, formattedSize))
        Spacer(modifier = Modifier.height(16.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Checkbox(checked = wifiOnly, onCheckedChange = { wifiOnly = it })
            Text(text = stringResource(R.string.installer_wait_for_wifi_checkbox_text))
        }
        Spacer(modifier = Modifier.height(16.dp))
        Button(onClick = { onInstallClick(wifiOnly) }) {
            Text(text = stringResource(R.string.installer_install_button_enabled_text))
        }
    }
}

@Composable
fun InstallProgressScreen(
    progressFlow: StateFlow<Long>,
    totalBytes: Long,
    onCancelClick: () -> Unit,
) {
    val currentBytes by progressFlow.collectAsStateWithLifecycle()
    Column(
        modifier = Modifier.fillMaxSize().padding(16.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        val progress = if (totalBytes > 0) currentBytes.toFloat() / totalBytes else 0f
        LinearProgressIndicator(progress = { progress })
        Spacer(modifier = Modifier.height(16.dp))
        Button(onClick = onCancelClick) { Text(text = stringResource(android.R.string.cancel)) }
    }
}

@Composable
fun BootingScreen() {
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator()
    }
}

private suspend fun handleError(
    context: Context,
    activity: Activity?,
    snackbarHostState: SnackbarHostState,
    viewModel: MainViewModel,
    state: MainUiState.Error,
) {
    val (messageId, actionLabel, action) =
        when (val handler = state.handler) {
            MainUiState.ErrorHandler.CheckNetwork ->
                Triple(
                    R.string.installer_error_no_wifi,
                    context.getString(R.string.action_settings),
                    { context.startActivity(Intent(Settings.ACTION_WIFI_SETTINGS)) },
                )
            MainUiState.ErrorHandler.Retry ->
                Triple(R.string.installer_error_unknown, "Retry", { viewModel.installVm() })
            is MainUiState.ErrorHandler.ReportBug ->
                Triple(
                    R.string.vm_error_message,
                    context.getString(R.string.error_button_report_bug),
                    {
                        if (activity != null) {
                            val error = handler.error
                            val exception = error as? Exception ?: Exception(error)
                            BetterBugLauncher.launchBetterBugActivity(activity, exception)
                        }
                    },
                )
        }

    val result =
        snackbarHostState.showSnackbar(
            message = context.getString(messageId),
            actionLabel = actionLabel,
            duration = SnackbarDuration.Indefinite,
        )
    if (result == SnackbarResult.ActionPerformed) {
        action()
    }
}

private fun onInstallClick(
    context: Context,
    scope: CoroutineScope,
    snackbarHostState: SnackbarHostState,
    viewModel: MainViewModel,
    wifiOnly: Boolean,
) {
    if (wifiOnly && !isWifiConnected(context)) {
        scope.launch {
            val result =
                snackbarHostState.showSnackbar(
                    message = context.getString(R.string.installer_error_no_wifi),
                    actionLabel = context.getString(R.string.action_settings),
                    duration = SnackbarDuration.Short,
                )
            if (result == SnackbarResult.ActionPerformed) {
                context.startActivity(Intent(Settings.ACTION_WIFI_SETTINGS))
            }
        }
    } else {
        viewModel.installVm()
    }
}

private fun isWifiConnected(context: Context): Boolean {
    val connectivityManager =
        context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    val network = connectivityManager.activeNetwork ?: return false
    val capabilities = connectivityManager.getNetworkCapabilities(network) ?: return false
    return capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
}
