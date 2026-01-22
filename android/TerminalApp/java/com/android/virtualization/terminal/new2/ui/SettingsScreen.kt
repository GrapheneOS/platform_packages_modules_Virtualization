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
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi
import androidx.compose.material3.adaptive.layout.AnimatedPane
import androidx.compose.material3.adaptive.layout.ListDetailPaneScaffoldRole
import androidx.compose.material3.adaptive.navigation.NavigableListDetailPaneScaffold
import androidx.compose.material3.adaptive.navigation.rememberListDetailPaneScaffoldNavigator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.android.virtualization.terminal.GraphicsManager
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.core.OpenPort
import com.android.virtualization.terminal.new2.core.VmController
import com.android.virtualization.terminal.new2.ui.main.MainViewModel
import kotlinx.coroutines.launch

enum class SettingsDestination(val title: Int, val icon: Int) {
    PortControl(R.string.settings_port_title, R.drawable.baseline_call_missed_outgoing_24),
    Graphics(R.string.settings_graphics_title, R.drawable.ic_display),
    Recovery(R.string.settings_recovery_title, R.drawable.baseline_settings_backup_restore_24),
}

@OptIn(ExperimentalMaterial3AdaptiveApi::class)
@Composable
fun SettingsScreen(onBack: () -> Unit, viewModel: MainViewModel = viewModel()) {
    val navigator = rememberListDetailPaneScaffoldNavigator<SettingsDestination>()
    val scope = rememberCoroutineScope()
    val configuration = LocalConfiguration.current
    val isMobileMode = configuration.screenWidthDp < 600
    val settingsRequest by viewModel.settingsRequest.collectAsStateWithLifecycle()

    val destinations = remember {
        SettingsDestination.values().filter {
            it != SettingsDestination.Graphics || VmController.isGraphicsAccelerationSupported
        }
    }

    LaunchedEffect(settingsRequest, isMobileMode) {
        if (settingsRequest != null) {
            navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, settingsRequest!!)
        } else if (!isMobileMode && navigator.currentDestination == null) {
            destinations.firstOrNull()?.let {
                navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, it)
            }
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
                AnimatedPane {
                    SettingsListPane(
                        destinations = destinations,
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
                AnimatedPane {
                    val destination =
                        (navigator.currentDestination?.contentKey as? SettingsDestination)
                            ?: if (!isMobileMode) destinations.firstOrNull() else null
                    if (destination != null) {
                        SettingsDetailPane(
                            destination = destination,
                            isMobileMode = isMobileMode,
                            onBack = { scope.launch { navigator.navigateBack() } },
                            onCloseSettings = onBack,
                        )
                    }
                }
            },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsListPane(
    destinations: List<SettingsDestination>,
    onItemClick: (SettingsDestination) -> Unit,
    selectedItem: SettingsDestination?,
    onBack: () -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings_title)) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(
                            Icons.AutoMirrored.Filled.ArrowBack,
                            contentDescription = stringResource(R.string.settings_btn_back_desc),
                        )
                    }
                },
            )
        }
    ) { innerPadding ->
        LazyColumn(modifier = Modifier.padding(innerPadding)) {
            items(destinations) { item ->
                val isSelected = selectedItem == item
                ListItem(
                    headlineContent = { Text(stringResource(item.title)) },
                    leadingContent = {
                        Icon(painter = painterResource(item.icon), contentDescription = null)
                    },
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
    onCloseSettings: () -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(destination.title)) },
                navigationIcon = {
                    if (isMobileMode) {
                        IconButton(onClick = onBack) {
                            Icon(
                                Icons.AutoMirrored.Filled.ArrowBack,
                                contentDescription = stringResource(R.string.settings_btn_back_desc),
                            )
                        }
                    }
                },
            )
        }
    ) { innerPadding ->
        Box(modifier = Modifier.padding(innerPadding)) {
            when (destination) {
                SettingsDestination.PortControl -> PortControlPage()
                SettingsDestination.Graphics -> GraphicsAccelerationPage(onCloseSettings)
                SettingsDestination.Recovery -> RecoveryPage()
            }
        }
    }
}

@Composable
fun GraphicsAccelerationPage(onCloseSettings: () -> Unit, viewModel: MainViewModel = viewModel()) {
    val currentType = VmController.graphicsAccelerationType
    var showSelectionDialog by remember { mutableStateOf(false) }
    var showRebootDialog by remember { mutableStateOf(false) }
    var selectedType by remember { mutableStateOf(currentType) }

    val typeToName =
        mapOf(
            GraphicsManager.AccelerationType.Lavapipe to
                stringResource(R.string.settings_graphics_renderer_software),
            GraphicsManager.AccelerationType.Gfxstream to
                stringResource(R.string.settings_graphics_renderer_gpu),
        )

    if (showSelectionDialog) {
        AlertDialog(
            onDismissRequest = { showSelectionDialog = false },
            title = { Text(stringResource(R.string.settings_graphics_title)) },
            text = {
                Column(Modifier.selectableGroup()) {
                    GraphicsManager.AccelerationType.values().forEach { type ->
                        Row(
                            Modifier.fillMaxWidth()
                                .height(56.dp)
                                .selectable(
                                    selected = (type == selectedType),
                                    onClick = { selectedType = type },
                                    role = Role.RadioButton,
                                )
                                .padding(horizontal = 16.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            RadioButton(selected = (type == selectedType), onClick = null)
                            Text(
                                text = typeToName[type] ?: "",
                                style = MaterialTheme.typography.bodyLarge,
                                modifier = Modifier.padding(start = 16.dp),
                            )
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        showSelectionDialog = false
                        if (currentType != selectedType) {
                            VmController.setGraphicsAccelerationType(selectedType)
                            showRebootDialog = true
                        }
                    }
                ) {
                    Text(stringResource(android.R.string.ok))
                }
            },
            dismissButton = {
                TextButton(onClick = { showSelectionDialog = false }) {
                    Text(stringResource(android.R.string.cancel))
                }
            },
        )
    }

    if (showRebootDialog) {
        AlertDialog(
            onDismissRequest = { showRebootDialog = false },
            title = { Text(stringResource(R.string.settings_graphics_dlg_title_restart)) },
            text = { Text(stringResource(R.string.settings_graphics_dlg_message_restart)) },
            confirmButton = {
                TextButton(
                    onClick = {
                        showRebootDialog = false
                        viewModel.restartVm()
                        onCloseSettings()
                    }
                ) {
                    Text(stringResource(R.string.settings_graphics_dlg_btn_restart))
                }
            },
            dismissButton = {
                TextButton(onClick = { showRebootDialog = false }) {
                    Text(stringResource(R.string.settings_graphics_dlg_btn_later))
                }
            },
        )
    }

    LazyColumn(modifier = Modifier.fillMaxSize()) {
        item {
            ListItem(
                headlineContent = { Text(stringResource(R.string.settings_graphics_title)) },
                supportingContent = { Text(typeToName[currentType] ?: "") },
                modifier =
                    Modifier.clickable {
                        selectedType = currentType
                        showSelectionDialog = true
                    },
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PortControlPage() {
    val ports by VmController.ports.collectAsState()
    var showAddDialog by remember { mutableStateOf(false) }

    val activePorts = ports.filter { !it.isSaved() }
    val savedPorts = ports.filter { it.isSaved() }

    if (showAddDialog) {
        AddPortDialog(
            onDismissRequest = { showAddDialog = false },
            onConfirm = { port ->
                VmController.enablePortForwarding(port, true)
                showAddDialog = false
            },
            ports = ports,
        )
    }

    Scaffold(
        floatingActionButton = {
            FloatingActionButton(onClick = { showAddDialog = true }) {
                Icon(
                    painter = painterResource(R.drawable.ic_add),
                    contentDescription = stringResource(R.string.settings_port_btn_add),
                )
            }
        }
    ) { innerPadding ->
        if (ports.isEmpty()) {
            Box(
                modifier = Modifier.fillMaxSize().padding(innerPadding),
                contentAlignment = Alignment.Center,
            ) {
                Text(stringResource(R.string.settings_port_message_empty))
            }
        } else {
            LazyColumn(modifier = Modifier.fillMaxSize().padding(innerPadding)) {
                if (activePorts.isNotEmpty()) {
                    item {
                        Text(
                            text = stringResource(R.string.settings_port_title_active_ports),
                            style = MaterialTheme.typography.titleMedium,
                            modifier = Modifier.padding(16.dp),
                        )
                    }
                    items(activePorts) { port ->
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

                if (savedPorts.isNotEmpty()) {
                    item {
                        Text(
                            text = stringResource(R.string.settings_port_title_saved_ports),
                            style = MaterialTheme.typography.titleMedium,
                            modifier = Modifier.padding(16.dp),
                        )
                    }
                    items(savedPorts) { port ->
                        ListItem(
                            headlineContent = { Text(port.port.toString()) },
                            trailingContent = {
                                IconButton(
                                    onClick = {
                                        VmController.enablePortForwarding(port.port, false)
                                    }
                                ) {
                                    Icon(
                                        painter = painterResource(R.drawable.ic_close),
                                        contentDescription =
                                            stringResource(
                                                R.string.settings_port_btn_delete,
                                                port.port,
                                            ),
                                    )
                                }
                            },
                        )
                    }
                }
            }
        }
    }
}

@Composable
fun AddPortDialog(onDismissRequest: () -> Unit, onConfirm: (Int) -> Unit, ports: List<OpenPort>) {
    var portToAdd by remember { mutableStateOf("") }
    var portErrorResId by remember { mutableStateOf<Int?>(null) }

    AlertDialog(
        onDismissRequest = onDismissRequest,
        title = { Text(stringResource(R.string.settings_port_dlg_title_add)) },
        text = {
            Column {
                OutlinedTextField(
                    value = portToAdd,
                    onValueChange = {
                        portToAdd = it
                        portErrorResId = null
                    },
                    label = { Text(stringResource(R.string.settings_port_dlg_hint_port_number)) },
                    isError = portErrorResId != null,
                    supportingText = portErrorResId?.let { { Text(stringResource(it)) } },
                    modifier = Modifier.fillMaxWidth(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                )
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    val port = portToAdd.toIntOrNull()
                    if (port == null) {
                        portErrorResId = R.string.settings_port_dlg_error_invalid_input
                    } else if (port < 1024 || port > 65535) {
                        portErrorResId = R.string.settings_port_dlg_error_invalid_range
                    } else if (ports.any { it.port == port }) {
                        portErrorResId = R.string.settings_port_dlg_error_existing
                    } else {
                        onConfirm(port)
                    }
                }
            ) {
                Text(stringResource(R.string.settings_port_dlg_btn_save))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismissRequest) {
                Text(stringResource(R.string.settings_port_dlg_btn_cancel))
            }
        },
    )
}
