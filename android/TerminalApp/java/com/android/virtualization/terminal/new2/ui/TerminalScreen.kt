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

import android.content.res.Configuration
import android.view.KeyEvent
import android.view.ViewConfiguration
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.indication
import androidx.compose.foundation.interaction.MutableInteractionSource
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
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowUp
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.android.virtualization.terminal.new2.core.TerminalSession
import com.android.virtualization.terminal.new2.ui.main.TerminalUiState
import com.android.virtualization.terminal.new2.ui.main.TerminalViewModel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

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
        modifier = Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface),
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

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun TerminalScreen(address: String, port: Int, tabId: String) {
    val terminalViewModel: TerminalViewModel = viewModel(key = tabId)
    val terminalUiState by terminalViewModel.uiState.collectAsStateWithLifecycle()
    val ttydView =
        remember(address, port, tabId) { terminalViewModel.getOrCreateTtydView(address, port) }
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
                        Text(text = "Connecting to terminal...")
                    }
                }
                is TerminalUiState.Disconnected -> {
                    Text(text = "Terminal disconnected.")
                }
                else -> {
                    Column(
                        modifier = Modifier.fillMaxSize(),
                        verticalArrangement = Arrangement.Center,
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        CircularProgressIndicator()
                        Spacer(modifier = Modifier.height(16.dp))
                        Text(text = "Initializing terminal...")
                    }
                }
            }
        }
        if (WindowInsets.isImeVisible) {
            TerminalKeys(
                onKeyAction = { key, action ->
                    if (key == ExtraKey.CTRL) {
                        if (action == KeyEvent.ACTION_DOWN) {
                            ttydView.mapCtrlKey()
                            ttydView.enableCtrlKey()
                        }
                    } else {
                        key.keyCode?.let { code ->
                            ttydView.dispatchKeyEvent(KeyEvent(action, code))
                        }
                    }
                }
            )
        }
    }
}

enum class ExtraKey(val label: String, val icon: ImageVector? = null, val keyCode: Int? = null) {
    ESC("Esc", keyCode = KeyEvent.KEYCODE_ESCAPE),
    TAB("Tab", keyCode = KeyEvent.KEYCODE_TAB),
    HOME("Home", keyCode = KeyEvent.KEYCODE_MOVE_HOME),
    UP("Up", icon = Icons.Default.KeyboardArrowUp, keyCode = KeyEvent.KEYCODE_DPAD_UP),
    END("End", keyCode = KeyEvent.KEYCODE_MOVE_END),
    PGUP("PgUp", keyCode = KeyEvent.KEYCODE_PAGE_UP),
    CTRL("Ctrl"),
    ALT("Alt", keyCode = KeyEvent.KEYCODE_ESCAPE),
    LEFT("Left", icon = Icons.AutoMirrored.Filled.ArrowBack, keyCode = KeyEvent.KEYCODE_DPAD_LEFT),
    DOWN("Down", icon = Icons.Default.KeyboardArrowDown, keyCode = KeyEvent.KEYCODE_DPAD_DOWN),
    RIGHT(
        "Right",
        icon = Icons.AutoMirrored.Filled.ArrowForward,
        keyCode = KeyEvent.KEYCODE_DPAD_RIGHT,
    ),
    PGDN("PgDn", keyCode = KeyEvent.KEYCODE_PAGE_DOWN),
}

@Composable
fun TerminalKeys(onKeyAction: (ExtraKey, Int) -> Unit) {
    val configuration = LocalConfiguration.current
    val isLandscape = configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
    val keys =
        if (isLandscape) {
            listOf(
                ExtraKey.ESC,
                ExtraKey.TAB,
                ExtraKey.CTRL,
                ExtraKey.ALT,
                ExtraKey.HOME,
                ExtraKey.END,
                ExtraKey.LEFT,
                ExtraKey.DOWN,
                ExtraKey.UP,
                ExtraKey.RIGHT,
                ExtraKey.PGDN,
                ExtraKey.PGUP,
            )
        } else {
            listOf(
                ExtraKey.ESC,
                ExtraKey.TAB,
                ExtraKey.HOME,
                ExtraKey.UP,
                ExtraKey.END,
                ExtraKey.PGUP,
                ExtraKey.CTRL,
                ExtraKey.ALT,
                ExtraKey.LEFT,
                ExtraKey.DOWN,
                ExtraKey.RIGHT,
                ExtraKey.PGDN,
            )
        }
    val rows = if (isLandscape) 1 else 2
    val columns = keys.size / rows

    Column(
        modifier = Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant)
    ) {
        keys.chunked(columns).forEach { rowKeys ->
            Row(modifier = Modifier.fillMaxWidth()) {
                rowKeys.forEach { key ->
                    Box(
                        modifier =
                            Modifier.weight(1f)
                                .height(40.dp)
                                .repeatingClickable(
                                    interactionSource = remember { MutableInteractionSource() },
                                    enabled = true,
                                    onPress = { onKeyAction(key, KeyEvent.ACTION_DOWN) },
                                    onRelease = { onKeyAction(key, KeyEvent.ACTION_UP) },
                                ),
                        contentAlignment = Alignment.Center,
                    ) {
                        if (key.icon != null) {
                            Icon(key.icon, contentDescription = key.label)
                        } else {
                            Text(text = key.label, style = MaterialTheme.typography.labelSmall)
                        }
                    }
                }
            }
        }
    }
}

fun Modifier.repeatingClickable(
    interactionSource: MutableInteractionSource,
    enabled: Boolean,
    onPress: () -> Unit,
    onRelease: () -> Unit,
): Modifier = composed {
    val currentPressListener by rememberUpdatedState(onPress)
    val currentReleaseListener by rememberUpdatedState(onRelease)
    val scope = rememberCoroutineScope()

    val initialDelay = remember { ViewConfiguration.getKeyRepeatTimeout().toLong() }
    val repeatDelay = remember { ViewConfiguration.getKeyRepeatDelay().toLong() }

    this.pointerInput(interactionSource, enabled) {
            awaitEachGesture {
                awaitFirstDown(requireUnconsumed = false)
                val job =
                    scope.launch {
                        // Initial press
                        currentPressListener()
                        delay(initialDelay)
                        while (enabled) {
                            currentPressListener()
                            delay(repeatDelay)
                        }
                    }
                waitForUpOrCancellation()
                job.cancel()
                currentReleaseListener()
            }
        }
        .indication(interactionSource, androidx.compose.foundation.LocalIndication.current)
}
