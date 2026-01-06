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
import android.graphics.Matrix
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceView
import android.view.View
import android.view.ViewGroup
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.widget.FrameLayout
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.VectorConverter
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.exclude
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Keyboard
import androidx.compose.material.icons.filled.Monitor
import androidx.compose.material.icons.filled.Mouse
import androidx.compose.material.icons.filled.PanTool
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material.icons.outlined.Monitor
import androidx.compose.material.icons.outlined.Mouse
import androidx.compose.material.icons.outlined.PanTool
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.IconToggleButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.PointerEvent
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.pointerInteropFilter
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.android.virtualization.terminal.DisplayProvider
import com.android.virtualization.terminal.DisplaySurfaceView
import com.android.virtualization.terminal.InputForwarder
import com.android.virtualization.terminal.R
import com.android.virtualization.terminal.new2.core.VmController
import com.android.virtualization.terminal.new2.ui.main.DisplayState
import com.android.virtualization.terminal.new2.ui.main.MainViewModel
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

@OptIn(ExperimentalLayoutApi::class, ExperimentalComposeUiApi::class)
@Composable
fun DisplayScreen(viewModel: MainViewModel, modifier: Modifier = Modifier) {
    val vm = VmController.virtualMachine ?: return
    val vmDisplayWidth = vm.config.customImageConfig?.displayConfig!!.width
    val vmDisplayHeight = vm.config.customImageConfig?.displayConfig!!.height

    var displaySurfaceView by remember { mutableStateOf<DisplaySurfaceView?>(null) }

    val isImeVisible by viewModel.isImeVisible.collectAsStateWithLifecycle()
    val isPanZoomMode by viewModel.isPanZoomMode.collectAsStateWithLifecycle()
    val isMouseLocked by viewModel.isMouseLocked.collectAsStateWithLifecycle()
    val isWindowImeVisible = WindowInsets.isImeVisible

    val density = LocalDensity.current
    val imeHeight by
        rememberUpdatedState(
            (WindowInsets.ime.getBottom(density) - WindowInsets.navigationBars.getBottom(density))
                .coerceAtLeast(0)
                .toFloat()
        )
    val modifierKeysHeight =
        if (LocalConfiguration.current.orientation == Configuration.ORIENTATION_LANDSCAPE) {
            with(density) { 40.dp.toPx() }
        } else {
            with(density) { 80.dp.toPx() }
        }
    val keyboardAreaHeight = if (imeHeight > 0) imeHeight + modifierKeysHeight else 0f

    LaunchedEffect(displaySurfaceView, isImeVisible) {
        val dsv = displaySurfaceView ?: return@LaunchedEffect
        if (isImeVisible) {
            dsv.post { dsv.showSoftInput() }
        } else {
            dsv.post { dsv.hideSoftInput() }
        }
    }

    LaunchedEffect(isWindowImeVisible, isImeVisible) {
        if (isImeVisible != isWindowImeVisible) {
            if (isImeVisible) {
                // Wait for the IME animation to finish before syncing the state.
                delay(500)
                viewModel.setIsImeVisible(false)
            } else {
                // Wait for the IME animation to finish before syncing the state.
                delay(500)
                viewModel.setIsImeVisible(true)
            }
        }
    }

    LaunchedEffect(isMouseLocked, displaySurfaceView) {
        val dsv = displaySurfaceView ?: return@LaunchedEffect
        if (isMouseLocked) {
            dsv.requestPointerCapture()
        } else {
            dsv.releasePointerCapture()
        }
    }

    Box(modifier = modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
        Viewport(
            keyboardAreaHeight = keyboardAreaHeight,
            isPanZoomMode = isPanZoomMode,
            onTouchEvent = { event, contentSize ->
                val matrix = Matrix()
                matrix.setScale(
                    vmDisplayWidth.toFloat() / contentSize.width,
                    vmDisplayHeight.toFloat() / contentSize.height,
                )
                event.transform(matrix)
                vm.sendMultiTouchEvent(event)
            },
        ) {
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { ctx ->
                    val container =
                        FrameLayout(ctx).apply {
                            descendantFocusability = ViewGroup.FOCUS_AFTER_DESCENDANTS
                        }
                    val mainView =
                        DisplaySurfaceView(ctx, null).apply {
                            layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT)
                            isFocusable = true
                            isFocusableInTouchMode = true
                            requestFocus()
                        }
                    val cursorView =
                        SurfaceView(ctx).apply {
                            layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT)
                        }
                    container.addView(mainView)
                    container.addView(cursorView)

                    DisplayProvider(mainView, cursorView, vmDisplayWidth, vmDisplayHeight)

                    // Use a dummy view for touch receiver to prevent InputForwarder from
                    // overwriting our listener
                    val dummyView = View(ctx)
                    val inputForwarder = InputForwarder(ctx, vm, dummyView, mainView, mainView)
                    container.tag = inputForwarder
                    displaySurfaceView = mainView

                    container
                },
                onRelease = { view -> (view.tag as? InputForwarder)?.cleanUp() },
            )
        }

        if (isWindowImeVisible) {
            Box(
                modifier =
                    Modifier.align(Alignment.BottomCenter)
                        .windowInsetsPadding(WindowInsets.ime.exclude(WindowInsets.navigationBars))
            ) {
                ModifierKeys(
                    onKeyAction = { key, action ->
                        val dsv = displaySurfaceView ?: return@ModifierKeys
                        val down = action == KeyEvent.ACTION_DOWN
                        key.keyCode?.let {
                            val scanCode = dsv.convertAndroidKeyCodeToEvdevScanCode(it)
                            if (scanCode != (-1).toShort()) {
                                vm.sendKeyEvent(scanCode, down)
                            }
                        }
                    }
                )
            }
        }
    }
}

@Composable
fun DisplayController(viewModel: MainViewModel) {
    val displayState by viewModel.displayState.collectAsStateWithLifecycle()
    val isDisplayActive = displayState == DisplayState.Normal
    val isImeVisible by viewModel.isImeVisible.collectAsStateWithLifecycle()
    val isPanZoomMode by viewModel.isPanZoomMode.collectAsStateWithLifecycle()
    val isMouseLocked by viewModel.isMouseLocked.collectAsStateWithLifecycle()

    Surface(
        modifier = Modifier.padding(end = 8.dp),
        shape = RoundedCornerShape(24.dp),
        color = if (isDisplayActive) MaterialTheme.colorScheme.surfaceVariant else Color.Transparent,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            AnimatedVisibility(visible = isDisplayActive) {
                Row {
                    DisplayControllerButton(
                        checked = isPanZoomMode,
                        onCheckedChange = { viewModel.setPanZoomMode(!isPanZoomMode) },
                        pressedIcon = Icons.Filled.PanTool,
                        unpressedIcon = Icons.Outlined.PanTool,
                        pressedContentDescription =
                            stringResource(R.string.dispctrl_hint_exit_zoom),
                        unpressedContentDescription =
                            stringResource(R.string.dispctrl_hint_enter_zoom),
                    )
                    DisplayControllerButton(
                        checked = isImeVisible,
                        onCheckedChange = { viewModel.setIsImeVisible(!isImeVisible) },
                        pressedIcon = Icons.Filled.Keyboard,
                        unpressedIcon = Icons.Outlined.Keyboard,
                        pressedContentDescription =
                            stringResource(R.string.dispctrl_hint_hide_keyboard),
                        unpressedContentDescription =
                            stringResource(R.string.dispctrl_hint_show_keyboard),
                    )
                    DisplayControllerButton(
                        checked = isMouseLocked,
                        onCheckedChange = { viewModel.setMouseLocked(!isMouseLocked) },
                        pressedIcon = Icons.Filled.Mouse,
                        unpressedIcon = Icons.Outlined.Mouse,
                        pressedContentDescription =
                            stringResource(R.string.dispctrl_hint_release_mouse),
                        unpressedContentDescription =
                            stringResource(R.string.dispctrl_hint_lock_mouse),
                    )
                }
            }

            DisplayControllerButton(
                checked = isDisplayActive,
                onCheckedChange = { viewModel.toggleDisplay() },
                pressedIcon = Icons.Filled.Close,
                unpressedIcon = Icons.Outlined.Monitor,
                pressedContentDescription = stringResource(R.string.dispctrl_hint_close),
                unpressedContentDescription = stringResource(R.string.dispctrl_hint_options),
            )
        }
    }
}

@Composable
fun DisplayControllerButton(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    pressedIcon: ImageVector,
    unpressedIcon: ImageVector,
    pressedContentDescription: String,
    unpressedContentDescription: String,
) {
    IconToggleButton(
        checked = checked,
        onCheckedChange = onCheckedChange,
        colors =
            IconButtonDefaults.iconToggleButtonColors(
                checkedContentColor = MaterialTheme.colorScheme.primary
            ),
    ) {
        Icon(
            imageVector = if (checked) pressedIcon else unpressedIcon,
            contentDescription =
                if (checked) pressedContentDescription else unpressedContentDescription,
            modifier = Modifier.size(24.dp),
        )
    }
}

@OptIn(ExperimentalComposeUiApi::class)
@Composable
fun Viewport(
    keyboardAreaHeight: Float,
    isPanZoomMode: Boolean,
    onTouchEvent: (MotionEvent, IntSize) -> Unit,
    content: @Composable () -> Unit,
) {
    var scale by remember { mutableStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    var size by remember { mutableStateOf(IntSize.Zero) }

    val scope = rememberCoroutineScope()
    var animationJob by remember { mutableStateOf<Job?>(null) }

    /** Updates the scale and offset based on the gesture event. */
    fun updateTransform(event: PointerEvent) {
        if (event.changes.any { it.isConsumed }) return

        val zoom = event.calculateZoom()
        val pan = event.calculatePan()
        val centroid = event.calculateCentroid(useCurrent = false)

        if (zoom == 1f && pan == Offset.Zero) return

        val currentScale = scale
        val newScale = (currentScale * zoom).coerceIn(1f, 10f)
        val zoomChange = newScale / currentScale
        scale = newScale

        // Center of the content is the origin where the zoom is done
        val viewportCenter = Offset(size.width / 2f, size.height / 2f)
        val zoomCenter = viewportCenter + offset
        val centroidAfterZoom = (centroid - zoomCenter) * zoomChange + zoomCenter

        // Adjust the offset to keep the centeroid at the same position
        val counterOffset = (centroid - centroidAfterZoom)
        offset = offset + pan + counterOffset
    }

    /** Animates the offset to remain within visible bounds. */
    suspend fun clampOffset() {
        val maxX = size.width * (scale - 1) / 2f
        val minX = -maxX
        val maxY = size.height * (scale - 1) / 2f
        val minY = -maxY - keyboardAreaHeight
        val targetOffset =
            Offset(x = offset.x.coerceIn(minX, maxX), y = offset.y.coerceIn(minY, maxY))
        animate(
            initialValue = offset,
            targetValue = targetOffset,
            typeConverter = Offset.VectorConverter,
            animationSpec = tween(durationMillis = 200),
        ) { value, _ ->
            offset = value
        }
    }

    LaunchedEffect(keyboardAreaHeight, size) {
        animationJob?.cancel()
        animationJob = launch { clampOffset() }
    }

    /** Detect pinch in/out and panning gestures and adjust scale and offset of the content */
    val gestureHandler: suspend androidx.compose.ui.input.pointer.PointerInputScope.() -> Unit = {
        if (isPanZoomMode) {
            // Manage the complete gesture lifecycle (Down -> Move -> Up) to synchronize inputs,
            // handle animation interruptions, and trigger settling on release.
            awaitEachGesture {
                awaitFirstDown(requireUnconsumed = false)
                animationJob?.cancel()
                do {
                    val event = awaitPointerEvent()
                    updateTransform(event)
                    event.changes.forEach {
                        if (it.positionChanged()) {
                            it.consume()
                        }
                    }
                } while (event.changes.any { it.pressed })
                animationJob = scope.launch { clampOffset() }
            }
        }
    }

    /**
     * Transform touch coordinates from screen space to the zoomed/panned VM display coordinate
     * space.
     */
    val touchHandler = { event: MotionEvent ->
        if (!isPanZoomMode) {
            val centerX = size.width / 2f
            val centerY = size.height / 2f

            val matrix = Matrix()
            val graphicsLayerMatrix = Matrix()
            graphicsLayerMatrix.preScale(scale, scale, centerX, centerY)
            graphicsLayerMatrix.postTranslate(offset.x, offset.y)
            val inverseMatrix = Matrix()
            if (graphicsLayerMatrix.invert(inverseMatrix)) {
                matrix.postConcat(inverseMatrix)
            }
            val transformedEvent = MotionEvent.obtain(event)
            transformedEvent.transform(matrix)
            try {
                onTouchEvent(transformedEvent, size)
            } finally {
                transformedEvent.recycle()
            }
            true
        } else {
            false
        }
    }

    Box(modifier = Modifier.fillMaxSize().clipToBounds()) {
        // Layer that does the translation for the content
        Box(
            modifier =
                Modifier.fillMaxSize()
                    .graphicsLayer(
                        scaleX = scale,
                        scaleY = scale,
                        translationX = offset.x,
                        translationY = offset.y,
                    )
        ) {
            content()
        }
        // Transparent layer that detects the gesture and adjusts the translation, and also does
        // the inverse translation for touch events
        Box(
            modifier =
                Modifier.fillMaxSize()
                    .onSizeChanged { size = it }
                    .pointerInteropFilter(onTouchEvent = touchHandler)
                    .pointerInput(isPanZoomMode, gestureHandler)
                    .background(Color.Transparent)
        )
    }
}
