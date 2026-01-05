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

import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.provider.Settings
import android.text.format.Formatter
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.ui.main.MainViewModel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

@Composable
fun InstallStartScreen(totalSizeBytes: Long, onInstallClick: (Boolean) -> Unit) {
    var wifiOnly by remember { mutableStateOf(true) }
    val context = LocalContext.current
    val formattedSize = Formatter.formatFileSize(context, totalSizeBytes)

    Column(
        modifier =
            Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
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
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Checkbox(
                checked = wifiOnly,
                onCheckedChange = { wifiOnly = it },
                modifier = Modifier.size(24.dp),
                colors =
                    CheckboxDefaults.colors(
                        checkedColor = MaterialTheme.colorScheme.primary,
                        uncheckedColor = MaterialTheme.colorScheme.outline,
                    ),
            )
            Spacer(modifier = Modifier.width(32.dp))
            Text(
                text = stringResource(R.string.installer_chkbox_wifi_only),
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        Spacer(modifier = Modifier.height(16.dp))
        Button(
            onClick = { onInstallClick(wifiOnly) },
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
        Spacer(modifier = Modifier.weight(1f))
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

fun onInstallClick(
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
                    message = context.getString(R.string.installer_snkbar_error_no_wifi),
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
