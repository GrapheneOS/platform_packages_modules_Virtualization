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

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi
import androidx.compose.material3.adaptive.layout.AnimatedPane
import androidx.compose.material3.adaptive.layout.ListDetailPaneScaffoldRole
import androidx.compose.material3.adaptive.navigation.NavigableListDetailPaneScaffold
import androidx.compose.material3.adaptive.navigation.rememberListDetailPaneScaffoldNavigator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.unit.dp
import com.android.virtualization.terminal.new2.core.VmController
import kotlinx.coroutines.launch

enum class SettingsDestination(val title: String) {
    PortControl("Port Control"),
    Recovery("Recovery"),
}

@OptIn(ExperimentalMaterial3AdaptiveApi::class)
@Composable
fun SettingsScreen(onBack: () -> Unit, initialDestination: SettingsDestination? = null) {
    val navigator = rememberListDetailPaneScaffoldNavigator<SettingsDestination>()
    val scope = rememberCoroutineScope()
    val configuration = LocalConfiguration.current
    val isMobileMode = configuration.screenWidthDp < 600

    androidx.compose.runtime.LaunchedEffect(initialDestination) {
        if (initialDestination != null) {
            navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, initialDestination)
        }
    }

    BackHandler {
        if (navigator.canNavigateBack()) {
            scope.launch { navigator.navigateBack() }
        } else {
            onBack()
        }
    }

    Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        NavigableListDetailPaneScaffold(
            navigator = navigator,
            listPane = {
                AnimatedPane { // Wrapped with AnimatedPane
                    SettingsListPane(
                        onItemClick = { item ->
                            scope.launch {
                                navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, item)
                            }
                        },
                        selectedItem =
                            navigator.currentDestination?.contentKey as? SettingsDestination,
                        onBack = onBack,
                    )
                }
            },
            detailPane = {
                AnimatedPane { // Wrapped with AnimatedPane
                    val destination =
                        navigator.currentDestination?.contentKey as? SettingsDestination
                    if (destination != null) {
                        SettingsDetailPane(
                            destination = destination,
                            isMobileMode = isMobileMode,
                            onBack = { scope.launch { navigator.navigateBack() } },
                        )
                    } else {
                        // Placeholder for when no item is selected in detail pane
                        Surface(
                            modifier = Modifier.fillMaxSize(),
                            color = MaterialTheme.colorScheme.surface,
                        ) {
                            Box(contentAlignment = Alignment.Center) { Text("Select a setting") }
                        }
                    }
                }
            },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsListPane(
    onItemClick: (SettingsDestination) -> Unit,
    selectedItem: SettingsDestination?,
    onBack: () -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Settings") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
            )
        }
    ) { innerPadding ->
        LazyColumn(modifier = Modifier.padding(innerPadding)) {
            items(SettingsDestination.values()) { item ->
                val isSelected = selectedItem == item
                ListItem(
                    headlineContent = { Text(item.title) },
                    modifier = Modifier.clickable { onItemClick(item) },
                    colors =
                        ListItemDefaults.colors(
                            containerColor =
                                if (isSelected) MaterialTheme.colorScheme.secondaryContainer
                                else MaterialTheme.colorScheme.surface
                        ),
                )
                HorizontalDivider()
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsDetailPane(
    destination: SettingsDestination,
    isMobileMode: Boolean,
    onBack: () -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(destination.title) },
                navigationIcon = {
                    if (isMobileMode) {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                        }
                    }
                },
            )
        }
    ) { innerPadding ->
        Box(modifier = Modifier.padding(innerPadding)) {
            when (destination) {
                SettingsDestination.PortControl -> PortControlPage()
                SettingsDestination.Recovery -> RecoveryPage()
            }
        }
    }
}

@Composable
fun PortControlPage() {
    val ports by VmController.ports.collectAsState()

    if (ports.isEmpty()) {
        Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text("No port is opened")
        }
    } else {
        LazyColumn(modifier = Modifier.fillMaxSize()) {
            item {
                Text(
                    text = "Listening ports",
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.padding(16.dp),
                )
            }
            items(ports) { port ->
                ListItem(
                    headlineContent = { Text("${port.port} (${port.name})") },
                    trailingContent = {
                        Switch(
                            checked = port.isForwarded,
                            onCheckedChange = { isChecked ->
                                VmController.enablePortForwarding(port.port, isChecked)
                            },
                        )
                    },
                )
            }
        }
    }
}
