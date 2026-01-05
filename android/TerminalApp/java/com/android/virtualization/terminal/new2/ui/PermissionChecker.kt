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

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.android.virtualization.terminal.new2.ui.main.MainUiState
import com.android.virtualization.terminal.new2.ui.main.MainViewModel
import kotlinx.coroutines.launch

val PERMISSIONS =
    arrayOf(Manifest.permission.POST_NOTIFICATIONS, Manifest.permission.ACCESS_LOCAL_NETWORK)

@Composable
fun PermissionChecker(viewModel: MainViewModel, snackbarHostState: SnackbarHostState) {
    val context = LocalContext.current
    val activity = context as Activity
    val scope = rememberCoroutineScope()
    var showPermissionRationale by remember { mutableStateOf(false) }
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()

    fun getPermissionLabel(permission: String): String {
        return try {
            val permissionInfo = context.packageManager.getPermissionInfo(permission, 0)
            permissionInfo.loadLabel(context.packageManager).toString()
        } catch (e: PackageManager.NameNotFoundException) {
            permission
        }
    }

    val settingsLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {
            viewModel.startVm()
        }

    val permissionLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            results ->
            if (results.all { it.value }) {
                snackbarHostState.currentSnackbarData?.dismiss()
                viewModel.onPermissionGranted()
            } else {
                val deniedPermissions = results.filter { !it.value }.keys
                val deniedLabels =
                    deniedPermissions.map { getPermissionLabel(it) }.joinToString(", ")

                scope.launch {
                    val result =
                        snackbarHostState.showSnackbar(
                            message =
                                "The following permissions are missing. Please grant them in Settings: $deniedLabels",
                            actionLabel = "Settings",
                            duration = SnackbarDuration.Long,
                        )
                    if (result == SnackbarResult.ActionPerformed) {
                        val intent =
                            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                                data = Uri.fromParts("package", context.packageName, null)
                            }
                        settingsLauncher.launch(intent)
                    }
                }
            }
        }

    LaunchedEffect(uiState) {
        if (uiState is MainUiState.PermissionRequired) {
            if (PERMISSIONS.any { activity.shouldShowRequestPermissionRationale(it) }) {
                showPermissionRationale = true
            } else {
                permissionLauncher.launch(PERMISSIONS)
            }
        }
    }

    if (showPermissionRationale) {
        val missingPermissions =
            PERMISSIONS.filter {
                context.checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED
            }
        val permissionLabels = missingPermissions.map { getPermissionLabel(it) }.joinToString(", ")

        AlertDialog(
            onDismissRequest = {
                showPermissionRationale = false
                viewModel.onPermissionDenied()
            },
            title = { Text("Permission Required") },
            text = {
                Text(
                    "Terminal requires the following permissions to function correctly: $permissionLabels"
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        showPermissionRationale = false
                        permissionLauncher.launch(PERMISSIONS)
                    }
                ) {
                    Text("OK")
                }
            },
        )
    }
}
