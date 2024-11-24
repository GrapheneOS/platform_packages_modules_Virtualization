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

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.FileUtils
import android.text.SpannableString
import android.text.Spanned
import android.text.TextUtils
import android.text.format.Formatter
import android.text.style.RelativeSizeSpan
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.isVisible
import com.google.android.material.button.MaterialButton
import com.google.android.material.slider.Slider
import java.util.regex.Pattern

class SettingsDiskResizeActivity : AppCompatActivity() {
    private val maxDiskSizeMb: Float = (16 shl 10).toFloat()
    private val numberPattern: Pattern = Pattern.compile("[\\d]*[\\٫.,]?[\\d]+");

    private fun bytesToMb(bytes: Long): Long {
        return bytes shr 20;
    }

    private fun mbToBytes(bytes: Long): Long {
        return bytes shl 20;
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.settings_disk_resize)
        val sharedPref =
            this.getSharedPreferences(getString(R.string.preference_file_key), Context.MODE_PRIVATE)
        var diskSizeMb =
            bytesToMb(
                sharedPref.getLong(
                    getString(R.string.preference_disk_size_key),
                    0
                )
            ).toFloat();
        val partition = InstallUtils.getRootfsFile(this)
        val minDiskSizeMb =
            bytesToMb(MainActivity.getMinFilesystemSize(partition)).toFloat()
                .coerceAtMost(diskSizeMb)

        val diskSizeText = findViewById<TextView>(R.id.settings_disk_resize_resize_gb_assigned)
        val diskMaxSizeText = findViewById<TextView>(R.id.settings_disk_resize_resize_gb_max)
        diskMaxSizeText.text = getString(R.string.settings_disk_resize_resize_gb_max_format,
            localizedFileSize(maxDiskSizeMb)
        );

        val diskSizeSlider = findViewById<Slider>(R.id.settings_disk_resize_disk_size_slider)
        diskSizeSlider.setValueTo(maxDiskSizeMb)
        val cancelButton = findViewById<MaterialButton>(R.id.settings_disk_resize_cancel_button)
        val resizeButton = findViewById<MaterialButton>(R.id.settings_disk_resize_resize_button)
        diskSizeSlider.valueFrom = minDiskSizeMb
        diskSizeSlider.valueTo = maxDiskSizeMb
        diskSizeSlider.value = diskSizeMb
        diskSizeSlider.stepSize =
            resources.getInteger(R.integer.disk_size_round_up_step_size_in_mb).toFloat()
        diskSizeSlider.setLabelFormatter { value: Float ->
            localizedFileSize(value)
        }
        diskSizeText.text = enlargeFontOfNumber(
            getString(R.string.settings_disk_resize_resize_gb_assigned_format,
                localizedFileSize(diskSizeMb)
            )
        )

        diskSizeSlider.addOnChangeListener { _, value, _ ->
            diskSizeText.text = enlargeFontOfNumber(
                getString(R.string.settings_disk_resize_resize_gb_assigned_format,
                localizedFileSize(value)))
            cancelButton.isVisible = true
            resizeButton.isVisible = true
        }
        cancelButton.setOnClickListener {
            diskSizeSlider.value = diskSizeMb
            cancelButton.isVisible = false
            resizeButton.isVisible = false
        }

        resizeButton.setOnClickListener {
            diskSizeMb = diskSizeSlider.value
            cancelButton.isVisible = false
            resizeButton.isVisible = false
            val editor = sharedPref.edit()
            editor.putLong(
                getString(R.string.preference_disk_size_key),
                mbToBytes(diskSizeMb.toLong())
            )
            editor.apply()

            // Restart terminal
            val intent =
                baseContext.packageManager.getLaunchIntentForPackage(baseContext.packageName)
            intent?.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK)
            finish()
            startActivity(intent)
        }
    }

    fun localizedFileSize(sizeMb: Float): String {
        // formatShortFileSize() uses SI unit (i.e. kB = 1000 bytes),
        // so covert sizeMb with "MB" instead of "MIB".
        val bytes = FileUtils.parseSize(sizeMb.toLong().toString() + "MB")
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