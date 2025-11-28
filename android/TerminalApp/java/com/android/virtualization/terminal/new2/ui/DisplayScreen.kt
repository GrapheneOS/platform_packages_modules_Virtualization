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

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import com.android.virtualization.terminal.R

@Composable
fun DisplayScreen(modifier: Modifier = Modifier) {
    Box(modifier = modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text("Display") }
}

@Composable
fun DisplayController(
    isDisplayActive: Boolean,
    onDisplayToggle: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Surface(
        modifier = modifier.padding(end = 8.dp),
        shape = RoundedCornerShape(24.dp),
        color = if (isDisplayActive) MaterialTheme.colorScheme.surfaceVariant else Color.Transparent,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            AnimatedVisibility(visible = isDisplayActive) {
                Row {
                    IconButton(onClick = { /* TODO */ }) {
                        Icon(
                            painter = painterResource(R.drawable.ic_keyboard),
                            contentDescription = "Keyboard",
                            modifier = Modifier.size(24.dp),
                        )
                    }
                    IconButton(onClick = { /* TODO */ }) {
                        Icon(
                            painter = painterResource(R.drawable.ic_mouse_lock),
                            contentDescription = "Mouse",
                            modifier = Modifier.size(24.dp),
                        )
                    }
                    IconButton(onClick = { /* TODO */ }) {
                        Icon(
                            painter = painterResource(R.drawable.ic_fullscreen),
                            contentDescription = "Fullscreen",
                            modifier = Modifier.size(24.dp),
                        )
                    }
                }
            }

            IconButton(onClick = onDisplayToggle) {
                if (isDisplayActive) {
                    Icon(
                        imageVector = Icons.Default.Close,
                        contentDescription = "Close display controls",
                    )
                } else {
                    Icon(
                        painter = painterResource(R.drawable.ic_display),
                        contentDescription = "Display options",
                        modifier = Modifier.size(24.dp),
                    )
                }
            }
        }
    }
}
