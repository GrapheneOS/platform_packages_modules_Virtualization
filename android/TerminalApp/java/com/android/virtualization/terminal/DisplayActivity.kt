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
package com.android.virtualization.terminal

import android.os.Bundle
import android.system.virtualmachine.VirtualMachineManager
import android.view.SurfaceView
import android.view.WindowInsets
import android.view.WindowInsetsController

class DisplayActivity : BaseActivity() {
    private lateinit var displayProvider: DisplayProvider

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_display)
        val mainView = findViewById<SurfaceView>(R.id.surface_view)
        val cursorView = findViewById<SurfaceView>(R.id.cursor_surface_view)
        makeFullscreen()
        // Connect the views to the VM
        displayProvider = DisplayProvider(mainView, cursorView)
        val vmm =
            applicationContext.getSystemService<VirtualMachineManager>(
                VirtualMachineManager::class.java
            )
        val debianVm = vmm.get("debian")
        if (debianVm != null) {
            InputForwarder(
                this,
                debianVm,
                findViewById(R.id.background_touch_view),
                findViewById(R.id.surface_view),
                findViewById(R.id.surface_view),
            )
        }
    }

    override fun onPause() {
        super.onPause()
        displayProvider.notifyDisplayIsGoingToInvisible()
    }

    private fun makeFullscreen() {
        val w = window
        w.setDecorFitsSystemWindows(false)
        val insetsCtrl = w.insetsController
        insetsCtrl?.hide(WindowInsets.Type.systemBars())
        insetsCtrl?.setSystemBarsBehavior(
            WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        )
    }
}
