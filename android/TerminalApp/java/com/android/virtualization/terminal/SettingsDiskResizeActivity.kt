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

import android.content.Intent
import android.icu.text.MeasureFormat
import android.icu.text.NumberFormat
import android.icu.util.Measure
import android.icu.util.MeasureUnit
import android.os.Bundle
import android.os.Environment
import android.text.SpannableString
import android.text.Spanned
import android.text.TextUtils
import android.text.style.RelativeSizeSpan
import android.view.View
import android.widget.SeekBar
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.isVisible
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import java.util.Locale
import java.util.regex.Pattern

class SettingsDiskResizeActivity : AppCompatActivity() {
    private val numberPattern: Pattern = Pattern.compile("[\\d]*[\\٫.,]?[\\d]+")
    private val defaultMaxDiskSizeMb: Long = 16 shl 10

    private var diskSizeStepMb: Long = 0
    private var diskSizeMb: Long = 0
    private lateinit var buttons: View
    private lateinit var cancelButton: View
    private lateinit var resizeButton: View
    private lateinit var diskSizeText: TextView
    private lateinit var diskSizeSlider: SeekBar

    private fun bytesToMb(bytes: Long): Long {
        return bytes shr 20
    }

    private fun mbToBytes(bytes: Long): Long {
        return bytes shl 20
    }

    private fun mbToProgress(bytes: Long): Int {
        return (bytes / diskSizeStepMb).toInt()
    }

    private fun progressToMb(progress: Int): Long {
        return progress * diskSizeStepMb
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.settings_disk_resize)

        diskSizeStepMb = 1L shl resources.getInteger(R.integer.disk_size_round_up_step_size_in_mb)

        val image = InstalledImage.getDefault(this)
        diskSizeMb = bytesToMb(image.getSize())
        val minDiskSizeMb = bytesToMb(image.getSmallestSizePossible()).coerceAtMost(diskSizeMb)
        val usableSpaceMb =
            bytesToMb(Environment.getDataDirectory().getUsableSpace()) and
                (diskSizeStepMb - 1).inv()
        val maxDiskSizeMb = defaultMaxDiskSizeMb.coerceAtMost(diskSizeMb + usableSpaceMb)

        diskSizeText = findViewById<TextView>(R.id.settings_disk_resize_resize_gb_assigned)!!
        val diskMaxSizeText = findViewById<TextView>(R.id.settings_disk_resize_resize_gb_max)
        diskMaxSizeText.text =
            getString(
                R.string.settings_disk_resize_resize_gb_max_format,
                localizedFileSize(maxDiskSizeMb, /* isShort= */ true),
            )

        buttons = findViewById<View>(R.id.buttons)
        diskSizeSlider = findViewById<SeekBar>(R.id.settings_disk_resize_disk_size_slider)!!
        cancelButton = findViewById<View>(R.id.settings_disk_resize_cancel_button)
        resizeButton = findViewById<View>(R.id.settings_disk_resize_resize_button)
        diskSizeSlider.min = mbToProgress(minDiskSizeMb)
        diskSizeSlider.max = mbToProgress(maxDiskSizeMb)
        diskSizeSlider.progress = mbToProgress(diskSizeMb)
        updateSliderText(diskSizeMb)

        diskSizeSlider.setOnSeekBarChangeListener(
            object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(seekBar: SeekBar, progress: Int, fromUser: Boolean) {
                    updateSliderText(progressToMb(progress))
                    buttons.isVisible = true
                    cancelButton.isVisible = true
                    resizeButton.isVisible = true
                }

                override fun onStartTrackingTouch(seekBar: SeekBar?) {
                    // no-op
                }

                override fun onStopTrackingTouch(seekBar: SeekBar?) {
                    // no-op
                }
            }
        )

        cancelButton.setOnClickListener { cancel() }

        resizeButton.setOnClickListener { showConfirmationDialog() }
    }

    fun cancel() {
        diskSizeSlider.progress = mbToProgress(diskSizeMb)
        buttons.isVisible = false
    }

    fun showConfirmationDialog() {
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.settings_disk_resize_title)
            .setMessage(R.string.settings_disk_resize_resize_confirm_dialog_message)
            .setPositiveButton(R.string.settings_disk_resize_resize_confirm_dialog_confirm) { _, _
                ->
                resize()
            }
            .setNegativeButton(R.string.settings_disk_resize_resize_cancel) { _, _ -> cancel() }
            .create()
            .show()
    }

    private fun resize() {
        diskSizeMb = progressToMb(diskSizeSlider.progress)
        buttons.isVisible = false

        // Restart terminal
        val intent = baseContext.packageManager.getLaunchIntentForPackage(baseContext.packageName)
        intent?.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK)
        intent?.putExtra(MainActivity.KEY_DISK_SIZE, mbToBytes(diskSizeMb))
        finish()
        startActivity(intent)
    }

    fun updateSliderText(sizeMb: Long) {
        diskSizeText.text =
            enlargeFontOfNumber(
                getString(
                    R.string.settings_disk_resize_resize_gb_assigned_format,
                    localizedFileSize(sizeMb, /* isShort= */ true),
                )
            )
        diskSizeSlider.stateDescription =
            getString(
                R.string.settings_disk_resize_resize_gb_assigned_format,
                localizedFileSize(sizeMb, /* isShort= */ false),
            )
    }

    fun localizedFileSize(sizeMb: Long, isShort: Boolean): String {
        val sizeGb = sizeMb / (1 shl 10).toFloat()
        val measure = Measure(sizeGb, MeasureUnit.GIGABYTE)

        val localeFromContext: Locale = resources.configuration.locales[0]
        val numberFormatter: NumberFormat = NumberFormat.getInstance(localeFromContext)
        numberFormatter.minimumFractionDigits = 1
        numberFormatter.maximumFractionDigits = 1

        val formatWidth =
            if (isShort) MeasureFormat.FormatWidth.SHORT else MeasureFormat.FormatWidth.WIDE
        val measureFormat: MeasureFormat =
            MeasureFormat.getInstance(localeFromContext, formatWidth, numberFormatter)
        return measureFormat.format(measure)
    }

    fun enlargeFontOfNumber(summary: CharSequence): CharSequence {
        if (TextUtils.isEmpty(summary)) {
            return ""
        }

        val matcher = numberPattern.matcher(summary)
        if (matcher.find()) {
            val spannableSummary = SpannableString(summary)
            spannableSummary.setSpan(
                RelativeSizeSpan(2f),
                matcher.start(),
                matcher.end(),
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
            return spannableSummary
        }
        return summary
    }
}
