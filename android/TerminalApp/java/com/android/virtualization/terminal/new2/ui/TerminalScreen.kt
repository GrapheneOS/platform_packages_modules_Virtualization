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

import android.view.KeyEvent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.exclude
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
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
import androidx.compose.material3.TabRowDefaults.tabIndicatorOffset
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.core.TerminalSession
import com.android.virtualization.terminal.new2.ui.main.MainViewModel
import com.android.virtualization.terminal.new2.ui.main.TerminalUiState
import com.android.virtualization.terminal.new2.ui.main.TerminalViewModel
import kotlinx.coroutines.delay

val TAB_BAR_HEIGHT = 50.dp

@Composable
fun TerminalTabBar(
    tabs: List<TerminalSession>,
    selectedTabId: String?,
    onTabSelected: (String) -> Unit,
    onTabClosed: (String) -> Unit,
    onAddTab: () -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier =
            Modifier.fillMaxWidth()
                .height(TAB_BAR_HEIGHT)
                .background(MaterialTheme.colorScheme.surface),
    ) {
        key(tabs.size) {
            val selectedTabIndex = tabs.indexOfFirst { it.id == selectedTabId }.coerceAtLeast(0)
            ScrollableTabRow(
                selectedTabIndex = selectedTabIndex,
                modifier = Modifier.fillMaxWidth(),
                edgePadding = 0.dp,
                containerColor = Color.Transparent,
                indicator = { tabPositions ->
                    if (selectedTabId != null && selectedTabIndex < tabPositions.size) {
                        androidx.compose.material3.TabRowDefaults.SecondaryIndicator(
                            Modifier.tabIndicatorOffset(tabPositions[selectedTabIndex])
                        )
                    }
                },
                divider = {},
            ) {
                tabs.forEachIndexed { index, tab ->
                    val tabViewModel: TerminalViewModel = viewModel(key = tab.id)
                    val isSelected = tab.id == selectedTabId
                    val isNextSelected =
                        if (index < tabs.lastIndex) {
                            tabs[index + 1].id == selectedTabId
                        } else {
                            false
                        }
                    TerminalTab(
                        tab = tab,
                        selected = isSelected,
                        showSeparator = !isSelected && !isNextSelected,
                        onTabSelected = { onTabSelected(tab.id) },
                        onTabClosed = { onTabClosed(tab.id) },
                        tabViewModel = tabViewModel,
                    )
                    if (index == tabs.lastIndex) {
                        Box(modifier = Modifier.padding(horizontal = 6.dp)) {
                            IconButton(onClick = onAddTab) {
                                Icon(
                                    imageVector = Icons.Default.Add,
                                    contentDescription = stringResource(R.string.terminal_hint_btn_add_tab),
                                    modifier = Modifier.size(24.dp),
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
    showSeparator: Boolean,
    onTabSelected: () -> Unit,
    onTabClosed: () -> Unit,
    tabViewModel: TerminalViewModel,
) {
    var showCloseDialog by remember { mutableStateOf(false) }
    val title by tabViewModel.title.collectAsStateWithLifecycle()
    val separatorColor = MaterialTheme.colorScheme.outlineVariant

    if (showCloseDialog) {
        AlertDialog(
            onDismissRequest = { showCloseDialog = false },
            title = { Text(stringResource(R.string.terminal_dlg_title_close_tab)) },
            text = { Text(stringResource(R.string.terminal_dlg_message_close_tab)) },
            confirmButton = {
                TextButton(
                    onClick = {
                        tabViewModel.terminalClose()
                        onTabClosed()
                        showCloseDialog = false
                    }
                ) {
                    Text(stringResource(R.string.terminal_dlg_btn_close))
                }
            },
            dismissButton = { TextButton(onClick = { showCloseDialog = false }) { Text(stringResource(android.R.string.cancel)) } },
        )
    }

    Tab(
        selected = selected,
        onClick = onTabSelected,
        modifier =
            Modifier.width(150.dp).drawWithContent {
                drawContent()
                if (showSeparator) {
                    drawLine(
                        color = separatorColor,
                        start = Offset(x = size.width, y = 12.dp.toPx()),
                        end = Offset(x = size.width, y = size.height - 12.dp.toPx()),
                        strokeWidth = 1.dp.toPx(),
                    )
                }
            },
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier =
                Modifier.fillMaxHeight()
                    .background(
                        if (selected) MaterialTheme.colorScheme.surfaceVariant
                        else Color.Transparent
                    )
                    .padding(horizontal = 8.dp, vertical = 12.dp),
        ) {
            Text(
                text = title,
                maxLines = 1,
                modifier =
                    Modifier.weight(1f)
                        .graphicsLayer { alpha = 0.99f }
                        .drawWithContent {
                            drawContent()
                            drawRect(
                                brush =
                                    Brush.horizontalGradient(
                                        0.8f to Color.Black,
                                        1f to Color.Transparent,
                                    ),
                                blendMode = BlendMode.DstIn,
                            )
                        },
            )
            IconButton(
                onClick = { showCloseDialog = true },
                modifier = Modifier.size(24.dp).padding(start = 4.dp),
            ) {
                Icon(imageVector = Icons.Default.Close, contentDescription = stringResource(R.string.terminal_hint_btn_close_tab))
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun TerminalScreen(
    address: String,
    port: Int,
    key: String?,
    tabId: String,
    mainViewModel: MainViewModel,
) {
    val terminalViewModel: TerminalViewModel = viewModel(key = tabId)
    val terminalUiState by terminalViewModel.uiState.collectAsStateWithLifecycle()
    val ttydView =
        remember(address, port, tabId) { terminalViewModel.getOrCreateTtydView(address, port, key) }

    val storedImeVisibility by mainViewModel.isImeVisible.collectAsStateWithLifecycle()
    val isWindowImeVisible = WindowInsets.isImeVisible

    // 1. Sync ViewModel state to UI (Show/Hide Keyboard)
    LaunchedEffect(tabId, storedImeVisibility, terminalUiState) {
        if (terminalUiState is TerminalUiState.Ready) {
            ttydView.post {
                if (storedImeVisibility) {
                    ttydView.showSoftInput()
                } else {
                    ttydView.hideSoftInput()
                }
            }
        }
    }

    // 2. Sync UI state to ViewModel (Update ViewModel when user changes keyboard state)
    LaunchedEffect(tabId, isWindowImeVisible, storedImeVisibility) {
        if (storedImeVisibility != isWindowImeVisible) {
            if (storedImeVisibility) {
                // ViewModel says visible, but Window says invisible.  This could be due to tab
                // switching or keyboard animation.  Wait a bit to see if it persists (meaning user
                // closed it).
                delay(500)
                // Check directly against current window state (needs recomposition to get fresh
                // value?  No, LaunchedEffect restarts if isWindowImeVisible changes.  So if we are
                // here, it means isWindowImeVisible didn't change to true within 500ms.  But we
                // cannot access 'current' value inside LaunchedEffect without snapshotFlow or
                // simple variable capture.  Actually, if isWindowImeVisible changes, this coroutine
                // is cancelled.  So if we reached here, isWindowImeVisible is still false.
                mainViewModel.setIsImeVisible(false)
            } else {
                // ViewModel says invisible, Window says visible.  User opened the keyboard.
                delay(500)
                mainViewModel.setIsImeVisible(true)
            }
        }
    }

    Column(
        modifier =
            Modifier.fillMaxSize()
                .windowInsetsPadding(WindowInsets.ime.exclude(WindowInsets.navigationBars))
    ) {
        Box(modifier = Modifier.weight(1f), contentAlignment = Alignment.Center) {
            when (terminalUiState) {
                is TerminalUiState.Ready -> {
                    key(tabId) {
                        DisposableEffect(ttydView) {
                            ttydView.onResume()
                            onDispose { ttydView.onPause() }
                        }
                        AndroidView(
                            factory = {
                                ttydView.apply {
                                    setOnFocusChangeListener { _, hasFocus ->
                                        if (!hasFocus) disableCtrlKey()
                                    }
                                }
                            }
                        )
                    }
                }
                is TerminalUiState.Connecting -> {
                    Column(
                        modifier = Modifier.fillMaxSize(),
                        verticalArrangement = Arrangement.Center,
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        CircularProgressIndicator()
                        Spacer(modifier = Modifier.height(16.dp))
                        Text(text = stringResource(R.string.terminal_message_connecting))
                    }
                }
                is TerminalUiState.Disconnected -> {
                    Text(text = stringResource(R.string.terminal_message_disconnected))
                }
                else -> {
                    Column(
                        modifier = Modifier.fillMaxSize(),
                        verticalArrangement = Arrangement.Center,
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        CircularProgressIndicator()
                        Spacer(modifier = Modifier.height(16.dp))
                        Text(text = stringResource(R.string.terminal_message_initializing))
                    }
                }
            }
        }
        if (WindowInsets.isImeVisible) {
            ModifierKeys(
                onKeyAction = { key, action ->
                    if (key == ExtraKey.CTRL) {
                        if (action == KeyEvent.ACTION_DOWN) {
                            ttydView.mapCtrlKey()
                            ttydView.enableCtrlKey()
                        }
                    } else {
                        // Many terminal emulators send esc for alt for historical reason. We should
                        // do the same.
                        val code = if (key == ExtraKey.ALT) KeyEvent.KEYCODE_ESCAPE else key.keyCode
                        code?.let { ttydView.dispatchKeyEvent(KeyEvent(action, it)) }
                    }
                }
            )
        }
    }
}
