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
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowUp
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

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
fun ModifierKeys(onKeyAction: (ExtraKey, Int) -> Unit) {
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
