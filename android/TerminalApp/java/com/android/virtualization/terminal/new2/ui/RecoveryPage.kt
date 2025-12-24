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

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.android.virtualization.terminal.new2.ui.main.MainViewModel

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RecoveryPage(viewModel: MainViewModel = viewModel()) {
    var showResetConfirmationDialog by remember { mutableStateOf(false) }
    var showRemoveBackupDialog by remember { mutableStateOf(false) }
    val hasBackup by viewModel.hasBackup.collectAsStateWithLifecycle()

    Column(modifier = Modifier.fillMaxSize()) {
        ListItem(
            headlineContent = { Text("Reset to initial version") },
            modifier = Modifier.fillMaxWidth().clickable { showResetConfirmationDialog = true },
        )
        HorizontalDivider()

        if (hasBackup) {
            ListItem(
                headlineContent = { Text("Remove backup data") },
                modifier = Modifier.fillMaxWidth().clickable { showRemoveBackupDialog = true },
            )
            HorizontalDivider()
        }
    }

    if (showResetConfirmationDialog) {
        var backupDataChecked by remember { mutableStateOf(false) }
        AlertDialog(
            onDismissRequest = { showResetConfirmationDialog = false },
            title = { Text("Reset Terminal?") },
            text = {
                Column {
                    Text("Are you sure you want to reset Terminal to its initial version?")
                    Text("You will have to download the Terminal image again.")
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Checkbox(
                            checked = backupDataChecked,
                            onCheckedChange = { backupDataChecked = it },
                        )
                        Text(
                            text = "Backup data to /mnt/backup",
                            modifier = Modifier.clickable { backupDataChecked = !backupDataChecked },
                        )
                    }
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        viewModel.uninstallVm(backupDataChecked)
                        showResetConfirmationDialog = false
                    }
                ) {
                    Text("Reset")
                }
            },
            dismissButton = {
                TextButton(onClick = { showResetConfirmationDialog = false }) { Text("Cancel") }
            },
        )
    }

    if (showRemoveBackupDialog) {
        AlertDialog(
            onDismissRequest = { showRemoveBackupDialog = false },
            title = { Text("Remove backup data") },
            text = { Text("Remove /mnt/backup") },
            confirmButton = {
                TextButton(
                    onClick = {
                        viewModel.deleteBackup()
                        showRemoveBackupDialog = false
                    }
                ) {
                    Text("OK")
                }
            },
            dismissButton = {
                TextButton(onClick = { showRemoveBackupDialog = false }) { Text("Cancel") }
            },
        )
    }
}
