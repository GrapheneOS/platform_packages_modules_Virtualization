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
import android.os.FileUtils
import android.widget.TextView
import android.widget.Toast
import android.text.style.RelativeSizeSpan
import android.text.Spannable
import android.text.SpannableString
import android.text.Spanned
import android.text.format.Formatter
import android.text.TextUtils
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.isVisible
import com.google.android.material.button.MaterialButton
import com.google.android.material.slider.Slider

import java.util.regex.Matcher
import java.util.regex.Pattern

class SettingsDiskResizeActivity : AppCompatActivity() {
    private val maxDiskSize: Float = 256F
    private val numberPattern: Pattern = Pattern.compile("[\\d]*[\\٫.,]?[\\d]+");
    private var diskSize: Float = 104F

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.settings_disk_resize)
        val diskSizeText = findViewById<TextView>(R.id.settings_disk_resize_resize_gb_assigned)
        val diskMaxSizeText = findViewById<TextView>(R.id.settings_disk_resize_resize_gb_max)
        diskMaxSizeText.text = getString(R.string.settings_disk_resize_resize_gb_max_format,
            localizedFileSize(maxDiskSize));

        val diskSizeSlider = findViewById<Slider>(R.id.settings_disk_resize_disk_size_slider)
        diskSizeSlider.setValueTo(maxDiskSize)
        val cancelButton = findViewById<MaterialButton>(R.id.settings_disk_resize_cancel_button)
        val resizeButton = findViewById<MaterialButton>(R.id.settings_disk_resize_resize_button)
        diskSizeSlider.value = diskSize
        diskSizeText.text = enlargeFontOfNumber(
            getString(R.string.settings_disk_resize_resize_gb_assigned_format,
            localizedFileSize(diskSize)))

        diskSizeSlider.addOnChangeListener { _, value, _ ->
            diskSizeText.text = enlargeFontOfNumber(
                getString(R.string.settings_disk_resize_resize_gb_assigned_format,
                localizedFileSize(value)))
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

    fun localizedFileSize(sizeGb: Float): String {
        // formatShortFileSize() uses SI unit (i.e. kB = 1000 bytes),
        // so covert sizeGb with "GB" instead of "GIB".
        val bytes = FileUtils.parseSize(sizeGb.toLong().toString() + "GB")
        return Formatter.formatShortFileSize(this, bytes)
    }

    fun enlargeFontOfNumber(summary: CharSequence): CharSequence {
        if (TextUtils.isEmpty(summary)) {
            return ""
        }

        val matcher = numberPattern.matcher(summary);
        if (matcher.find()) {
            val spannableSummary = SpannableString(summary)
            spannableSummary.setSpan(
                    RelativeSizeSpan(2f),
                    matcher.start(),
                    matcher.end(),
                    Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            return spannableSummary
        }
        return summary
    }
}