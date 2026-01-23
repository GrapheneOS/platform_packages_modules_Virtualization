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
import android.text.format.Formatter
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.android.virtualization.terminal.ImageArchive
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.core.InstallState
import com.android.virtualization.terminal.new2.ui.main.InstallViewModel

@Composable
fun InstallScreen(snackbarHostState: SnackbarHostState, viewModel: InstallViewModel = viewModel()) {
    val state by viewModel.installState.collectAsStateWithLifecycle()
    val wifiOnly by viewModel.wifiOnly.collectAsStateWithLifecycle()

    var showCancelDialog by remember { mutableStateOf(false) }

    InstallErrorHandler(snackbarHostState, viewModel)

    if (showCancelDialog) {
        InstallCancelDialog(
            onConfirm = {
                showCancelDialog = false
                viewModel.cancelInstallVm()
            },
            onDismiss = { showCancelDialog = false },
        )
    }

    Column(
        modifier =
            Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        InstallScreenHeader(state.getTotalImageSize())

        if (!state.isStarted()) {
            InstallScreenButtons(
                wifiOnly = wifiOnly,
                onWifiOnlyChange = { viewModel.setWifiOnly(it) },
                onInstall = { viewModel.installVm() },
            )
        } else {
            InstallScreenProgress(
                state = state,
                wifiOnly = wifiOnly,
                onWifiOnlyChange = { viewModel.setWifiOnly(it) },
                onCancel = { showCancelDialog = true },
            )
        }
        Spacer(modifier = Modifier.weight(1f))
    }
}

@Composable
private fun InstallScreenHeader(totalSizeBytes: Long) {
    val context = LocalContext.current
    val formattedSize = Formatter.formatFileSize(context, totalSizeBytes)

    Spacer(modifier = Modifier.height(48.dp))
    Surface(
        shape = MaterialTheme.shapes.extraLarge,
        color = MaterialTheme.colorScheme.primaryContainer,
        shadowElevation = 2.dp,
    ) {
        Image(
            painter = painterResource(R.drawable.ic_launcher_foreground),
            contentDescription = null,
            modifier = Modifier.padding(16.dp).size(96.dp),
        )
    }
    Spacer(modifier = Modifier.height(16.dp))
    Text(
        text = stringResource(R.string.app_name),
        style = MaterialTheme.typography.headlineLarge,
        color = MaterialTheme.colorScheme.primary,
    )
    Spacer(modifier = Modifier.height(32.dp))
    Text(
        text = stringResource(R.string.installer_desc, formattedSize),
        style = MaterialTheme.typography.bodyLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
    )
    Spacer(modifier = Modifier.height(32.dp))
}

@Composable
private fun InstallScreenButtons(
    wifiOnly: Boolean,
    onWifiOnlyChange: (Boolean) -> Unit,
    onInstall: () -> Unit,
) {
    WifiOnlySwitch(wifiOnly = wifiOnly, onCheckedChange = onWifiOnlyChange)

    Spacer(modifier = Modifier.height(16.dp))
    Button(
        onClick = onInstall,
        modifier = Modifier.fillMaxWidth().height(56.dp),
        colors =
            ButtonDefaults.buttonColors(
                containerColor = MaterialTheme.colorScheme.primary,
                contentColor = MaterialTheme.colorScheme.onPrimary,
            ),
    ) {
        Text(
            text = stringResource(R.string.installer_btn_install),
            style = MaterialTheme.typography.labelLarge,
        )
    }
}

@Composable
private fun InstallScreenProgress(
    state: InstallState,
    wifiOnly: Boolean,
    onWifiOnlyChange: (Boolean) -> Unit,
    onCancel: () -> Unit,
) {
    val context = LocalContext.current
    val totalSizeBytes = state.getTotalImageSize()
    val formattedSize = Formatter.formatFileSize(context, totalSizeBytes)

    val currentBytesFlow: kotlinx.coroutines.flow.StateFlow<Long>?
    val progressColor: Color
    val message: Int?

    if (state is InstallState.Installing) {
        currentBytesFlow = state.progress
        progressColor = MaterialTheme.colorScheme.primary
        message = if (state.onWifi) null else R.string.installer_msg_mobile_data
    } else if (state is InstallState.InstallSuspended) {
        currentBytesFlow = state.progress
        progressColor = Color.Gray
        message = R.string.installer_msg_waiting_network
    } else {
        currentBytesFlow = null
        progressColor = Color.Transparent
        message = null
    }

    if (ImageArchive.isLocalImage()) {
        // No need to localization for userdebug only UX
        Text(text = "Auto installing from /sdcard/linux")
    }

    if (currentBytesFlow != null) {
        val currentBytes by currentBytesFlow.collectAsStateWithLifecycle()
        val progress = if (totalSizeBytes > 0) currentBytes.toFloat() / totalSizeBytes else 0f
        val formattedCurrent = Formatter.formatFileSize(context, currentBytes)

        Card(modifier = Modifier.fillMaxWidth()) {
            Column(modifier = Modifier.padding(16.dp)) {
                Box(modifier = Modifier.fillMaxWidth()) {
                    Text(
                        text = "$formattedCurrent / $formattedSize",
                        style = MaterialTheme.typography.labelSmall,
                        modifier = Modifier.align(Alignment.TopEnd),
                    )
                }
                LinearProgressIndicator(
                    progress = { progress },
                    modifier = Modifier.fillMaxWidth().height(4.dp),
                    color = progressColor,
                )

                if (message != null) {
                    Text(
                        text = stringResource(message),
                        style = MaterialTheme.typography.labelSmall,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }

                Spacer(modifier = Modifier.height(8.dp))

                WifiOnlySwitch(wifiOnly = wifiOnly, onCheckedChange = onWifiOnlyChange)
            }
        }
    }

    Spacer(modifier = Modifier.height(16.dp))
    TextButton(onClick = onCancel) {
        Text(
            text = stringResource(R.string.installer_btn_cancel),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun InstallCancelDialog(onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.installer_dlg_title_cancel_install)) },
        text = { Text(stringResource(R.string.installer_dlg_message_cancel_install)) },
        confirmButton = {
            TextButton(onClick = onConfirm) {
                Text(stringResource(R.string.installer_dlg_btn_cancel_install))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(stringResource(R.string.installer_dlg_btn_continue_install))
            }
        },
    )
}

@Composable
fun InstallErrorHandler(snackbarHostState: SnackbarHostState, viewModel: InstallViewModel) {
    val context = LocalContext.current
    val activity = context as Activity

    LaunchedEffect(viewModel.uiEvents) {
        viewModel.uiEvents.collect { event ->
            when (event) {
                is InstallViewModel.InstallUiEvent.ShowSnackbar -> {
                    val result =
                        snackbarHostState.showSnackbar(
                            message = activity.getString(event.messageId),
                            actionLabel = activity.getString(event.actionLabelId),
                            duration = SnackbarDuration.Indefinite,
                        )
                    if (result == SnackbarResult.ActionPerformed) {
                        viewModel.performAction(activity, event)
                    }
                }
            }
        }
    }
}

@Composable
fun WifiOnlySwitch(
    wifiOnly: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier.fillMaxWidth().clickable { onCheckedChange(!wifiOnly) },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            text = stringResource(R.string.installer_chkbox_wifi_only),
            style = MaterialTheme.typography.bodyMedium,
        )
        Switch(
            checked = wifiOnly,
            onCheckedChange = onCheckedChange,
            colors =
                SwitchDefaults.colors(
                    checkedThumbColor = MaterialTheme.colorScheme.onPrimary,
                    checkedTrackColor = MaterialTheme.colorScheme.primary,
                    uncheckedThumbColor = MaterialTheme.colorScheme.outline,
                    uncheckedTrackColor = MaterialTheme.colorScheme.surfaceVariant,
                ),
        )
    }
}
