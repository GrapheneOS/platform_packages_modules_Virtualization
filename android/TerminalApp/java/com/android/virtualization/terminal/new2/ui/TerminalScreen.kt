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

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ScrollableTabRow
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.android.virtualization.terminal.new2.core.TerminalSession
import com.android.virtualization.terminal.new2.ui.main.TerminalUiState
import com.android.virtualization.terminal.new2.ui.main.TerminalViewModel

@Composable
fun TerminalTabBar(
    tabs: List<TerminalSession>,
    selectedTabId: String,
    onTabSelected: (String) -> Unit,
    onTabClosed: (String) -> Unit,
    onAddTab: () -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface),
    ) {
        key(tabs.size) {
            ScrollableTabRow(
                selectedTabIndex = tabs.indexOfFirst { it.id == selectedTabId }.coerceAtLeast(0),
                modifier = Modifier.fillMaxWidth(),
                edgePadding = 0.dp,
                containerColor = Color.Transparent,
                divider = {},
            ) {
                tabs.forEachIndexed { index, tab ->
                    val tabViewModel: TerminalViewModel = viewModel(key = tab.id)
                    TerminalTab(
                        tab = tab,
                        selected = tab.id == selectedTabId,
                        onTabSelected = { onTabSelected(tab.id) },
                        onTabClosed = { onTabClosed(tab.id) },
                        tabViewModel = tabViewModel,
                    )
                    if (index == tabs.lastIndex) {
                        Box(modifier = Modifier.padding(horizontal = 6.dp)) {
                            IconButton(onClick = onAddTab, modifier = Modifier.width(24.dp)) {
                                Icon(
                                    imageVector = Icons.Default.Add,
                                    contentDescription = "Add tab",
                                    modifier = Modifier.size(12.dp),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun TerminalTab(
    tab: TerminalSession,
    selected: Boolean,
    onTabSelected: () -> Unit,
    onTabClosed: () -> Unit,
    tabViewModel: TerminalViewModel,
) {
    var showCloseDialog by remember { mutableStateOf(false) }

    if (showCloseDialog) {
        AlertDialog(
            onDismissRequest = { showCloseDialog = false },
            title = { Text("Close Tab?") },
            text = { Text("Are you sure you want to close this tab?") },
            confirmButton = {
                TextButton(
                    onClick = {
                        tabViewModel.terminalClose()
                        onTabClosed()
                        showCloseDialog = false
                    }
                ) {
                    Text("Close")
                }
            },
            dismissButton = { TextButton(onClick = { showCloseDialog = false }) { Text("Cancel") } },
        )
    }

    Tab(selected = selected, onClick = onTabSelected) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier =
                Modifier.fillMaxHeight()
                    .padding(end = 4.dp)
                    .background(
                        if (selected) MaterialTheme.colorScheme.surfaceVariant
                        else Color.Transparent
                    )
                    .padding(horizontal = 8.dp, vertical = 12.dp),
        ) {
            Text(tab.title)
            IconButton(
                onClick = { showCloseDialog = true },
                modifier = Modifier.size(24.dp).padding(start = 12.dp),
            ) {
                Icon(imageVector = Icons.Default.Close, contentDescription = "Close tab")
            }
        }
    }
}

@Composable
fun TerminalScreen(address: String, port: Int, tabId: String) {
    val terminalViewModel: TerminalViewModel = viewModel(key = tabId)
    val terminalUiState by terminalViewModel.uiState.collectAsStateWithLifecycle()
    val ttydView =
        remember(address, port, tabId) { terminalViewModel.getOrCreateTtydView(address, port) }

    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        when (terminalUiState) {
            is TerminalUiState.Ready -> {
                key(tabId) {
                    DisposableEffect(ttydView) {
                        ttydView.onResume()
                        onDispose { ttydView.onPause() }
                    }
                    AndroidView(factory = { ttydView })
                }
            }
            is TerminalUiState.Connecting -> {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator()
                    Spacer(modifier = Modifier.height(16.dp))
                    Text(text = "Connecting to terminal...")
                }
            }
            is TerminalUiState.Disconnected -> {
                Text(text = "Terminal disconnected.")
            }
            else -> {
                // Initializing state
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator()
                    Spacer(modifier = Modifier.height(16.dp))
                    Text(text = "Initializing terminal...")
                }
            }
        }
    }
}
