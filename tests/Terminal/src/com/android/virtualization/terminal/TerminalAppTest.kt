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

import android.app.Instrumentation
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.SystemProperties
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.android.microdroid.test.common.DeviceProperties
import com.android.microdroid.test.common.MetricsProcessor
import java.io.IOException
import java.lang.Exception
import java.util.ArrayList
import org.junit.After
import org.junit.Assert
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class TerminalAppTest {
    private lateinit var instr: Instrumentation
    private lateinit var targetContext: Context
    private lateinit var properties: DeviceProperties
    private val metricsProc = MetricsProcessor("avf_perf/terminal/")

    @Before
    fun setup() {
        instr = InstrumentationRegistry.getInstrumentation()
        targetContext = instr.targetContext
        properties =
            DeviceProperties.create(DeviceProperties.PropertyGetter { SystemProperties.get(it) })
        installVmImage()
    }

    private fun installVmImage() {
        val INSTALL_TIMEOUT_MILLIS: Long = 300000 // 5 min

        val intent = Intent(targetContext, InstallerActivity::class.java)
        intent.setFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val activity = instr.startActivitySync(intent)
        if (activity is InstallerActivity) {
            Assert.assertTrue(
                "Failed to install VM image",
                activity.waitForInstallCompleted(INSTALL_TIMEOUT_MILLIS),
            )
        }
    }

    @Test
    @Throws(Exception::class)
    fun boot() {
        val isNestedVirt = properties.isCuttlefish() || properties.isGoldfish()
        val BOOT_TIMEOUT_MILLIS =
            (if (isNestedVirt) 180000 else 30000).toLong() // 30 sec (or 3 min)

        val intent = Intent(targetContext, MainActivity::class.java)
        intent.setFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

        val start = System.currentTimeMillis()
        val activity = instr.startActivitySync(intent)
        if (activity is MainActivity) {
            Assert.assertTrue(
                "Failed to boot in 30s",
                activity.waitForBootCompleted(BOOT_TIMEOUT_MILLIS),
            )
        }
        val delay = System.currentTimeMillis() - start

        // TODO: measure multiple times?
        val measurements: MutableList<Long?> = ArrayList<Long?>()
        measurements.add(delay)
        val stats = metricsProc.computeStats(measurements, "boot", "ms")
        val bundle = Bundle()
        for (entry in stats.entries) {
            bundle.putDouble(entry.key, entry.value)
        }
        instr.sendStatus(0, bundle)
    }

    @After
    @Throws(IOException::class)
    fun tearDown() {
        PortsStateManager.getInstance(targetContext).clearEnabledPorts()
        InstalledImage.getDefault(targetContext).uninstallFully()
    }
}
