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

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat
import androidx.lifecycle.viewmodel.compose.viewModel
import com.android.virtualization.terminal.new2.core.VmService
import com.android.virtualization.terminal.new2.ui.main.MainViewModel

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Ensure the app handles insets manually and the window background is black
        // to prevent white flashes during IME animations or layout jumps.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        window.setBackgroundDrawableResource(android.R.color.black)

        setContent {
            val darkTheme = isSystemInDarkTheme()
            val context = LocalContext.current
            val colorScheme =
                if (darkTheme) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)

            val view = LocalView.current
            if (!view.isInEditMode) {
                SideEffect {
                    val window = (view.context as Activity).window
                    WindowCompat.getInsetsController(window, view).isAppearanceLightStatusBars =
                        !darkTheme
                }
            }

            MaterialTheme(colorScheme = colorScheme) {
                val viewModel: MainViewModel = viewModel()
                viewModel.handleIntent(intent)
                MainScreen(viewModel)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        val viewModel = androidx.lifecycle.ViewModelProvider(this)[MainViewModel::class.java]
        viewModel.handleIntent(intent)
    }

    override fun onStart() {
        super.onStart()
        val intent =
            Intent(this, VmService::class.java).apply { action = VmService.ACTION_APP_FOREGROUND }
        startForegroundService(intent)
    }

    override fun onStop() {
        super.onStop()
        val intent =
            Intent(this, VmService::class.java).apply { action = VmService.ACTION_APP_BACKGROUND }
        startForegroundService(intent)
    }

    companion object {
        const val ACTION_OPEN_SETTINGS_PORT =
            "android.virtualization.terminal.action.OPEN_SETTINGS_PORT"
    }
}
