/*
 * Copyright (C) 2024 The Android Open Source Project
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
import android.widget.TextView
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.isVisible
import com.google.android.material.button.MaterialButton
import com.google.android.material.slider.Slider

class SettingsDiskResizeActivity : AppCompatActivity() {
    private var diskSize: Float = 104F
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.settings_disk_resize)
        val diskSizeText = findViewById<TextView>(R.id.settings_disk_resize_disk_size)
        val diskSizeSlider = findViewById<Slider>(R.id.settings_disk_resize_disk_size_slider)
        val cancelButton = findViewById<MaterialButton>(R.id.settings_disk_resize_cancel_button)
        val resizeButton = findViewById<MaterialButton>(R.id.settings_disk_resize_resize_button)
        diskSizeText.text = diskSize.toInt().toString()
        diskSizeSlider.value = diskSize

        diskSizeSlider.addOnChangeListener { _, value, _ ->
            diskSizeText.text = value.toInt().toString()
            cancelButton.isVisible = true
            resizeButton.isVisible = true
        }
        cancelButton.setOnClickListener {
            diskSizeSlider.value = diskSize
            cancelButton.isVisible = false
            resizeButton.isVisible = false
        }

        resizeButton.setOnClickListener {
            diskSize = diskSizeSlider.value
            cancelButton.isVisible = false
            resizeButton.isVisible = false
            Toast.makeText(this@SettingsDiskResizeActivity, R.string.settings_disk_resize_resize_message, Toast.LENGTH_SHORT)
                .show()
        }
    }
}